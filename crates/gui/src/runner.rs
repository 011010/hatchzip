//! Runs one request at a time on a worker thread and keeps a [`Task`] up to
//! date for whichever window shows it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use slint::{Timer, TimerMode};

use crate::password;
use crate::task::Task;
use crate::work::{self, Counters, Request, Update};

/// About ten refreshes a second: smooth to the eye, and cheap however fast
/// the disk is, since the worker only bumps counters.
const TICK: Duration = Duration::from_millis(100);

thread_local! {
    // Updates come from the worker thread through the event loop, and only
    // `Send` data can make that trip, so they find the runner here.
    static RUNNER: RefCell<Option<Rc<Runner>>> = const { RefCell::new(None) };
}

pub struct Runner {
    pub task: RefCell<Option<Task>>,
    request: RefCell<Option<Request>>,
    counters: Arc<Counters>,
    timer: Timer,
    quit_when_stopped: Cell<bool>,
    show: Box<dyn Fn(&Task)>,
}

impl Runner {
    pub fn install(show: impl Fn(&Task) + 'static) -> Rc<Self> {
        let runner = Rc::new(Self {
            task: RefCell::new(None),
            request: RefCell::new(None),
            counters: Arc::new(Counters::default()),
            timer: Timer::default(),
            quit_when_stopped: Cell::new(false),
            show: Box::new(show),
        });
        RUNNER.with(|r| *r.borrow_mut() = Some(runner.clone()));
        runner
    }

    /// The password the last request opened its archive with, if any.
    pub fn password(&self) -> Option<String> {
        self.request
            .borrow()
            .as_ref()?
            .password()
            .map(str::to_string)
    }

    /// Stops a paused or failed extraction for good, giving the archive
    /// back. The engine's journal makes this safe at any point that isn't
    /// mid-run.
    pub fn give_back(&self) {
        if self.running() {
            return;
        }
        let Some(archive) = self
            .request
            .borrow()
            .as_ref()
            .map(|r| r.archive().to_path_buf())
        else {
            return;
        };
        let name = self
            .task
            .borrow()
            .as_ref()
            .map_or_else(String::new, |t| t.name().to_string());
        self.start(Task::restore(&name), Request::Restore { archive });
    }

    pub fn running(&self) -> bool {
        self.task.borrow().as_ref().is_some_and(Task::running)
    }

    pub fn start(&self, task: Task, request: Request) {
        self.start_after(|| {}, task, request);
    }

    /// Like [`Self::start`], with `before` run first on the worker thread,
    /// for cleanup that may have to wait on the disk.
    pub fn start_after(
        &self,
        before: impl FnOnce() + Send + 'static,
        task: Task,
        request: Request,
    ) {
        *self.task.borrow_mut() = Some(task);
        *self.request.borrow_mut() = Some(request);
        self.launch(before);
    }

    /// Runs the last request again: after a pause, a failure or freeing
    /// space. The engine picks up from its journal.
    pub fn retry(&self) {
        if self.running() || self.request.borrow().is_none() {
            return;
        }
        self.launch(|| {});
    }

    fn launch(&self, before: impl FnOnce() + Send + 'static) {
        let Some(request) = self.request.borrow().clone() else {
            return;
        };
        self.counters.reset();
        self.update(|t| t.apply(Update::Checking));
        let counters = self.counters.clone();
        let name = self
            .task
            .borrow()
            .as_ref()
            .map_or_else(String::new, |t| t.name().to_string());
        std::thread::spawn(move || {
            before();
            let send = |u: Update| {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(r) = RUNNER.with(|r| r.borrow().clone()) {
                        r.on_update(u);
                    }
                });
            };
            work::run(&request, &counters, &send, &mut |wrong| {
                password::ask(&name, wrong)
            });
        });
        self.timer.start(TimerMode::Repeated, TICK, || {
            if let Some(r) = RUNNER.with(|r| r.borrow().clone()) {
                r.tick();
            }
        });
    }

    pub fn pause(&self) {
        let mut asked = false;
        self.update(|t| asked = t.request_pause());
        if asked {
            self.counters.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Whether the window may close now. A run that is writing is paused
    /// first, and the app quits once the engine has stopped cleanly.
    pub fn may_close(&self) -> bool {
        if !self.running() {
            return true;
        }
        let writing =
            self.task.borrow().as_ref().is_some_and(|t| {
                matches!(t.view.phase, crate::Phase::Working | crate::Phase::Pausing)
            });
        if !writing {
            // Checking and testing write nothing, or nothing a rerun doesn't
            // already recover from, so cutting them short is safe.
            return true;
        }
        self.quit_when_stopped.set(true);
        self.pause();
        false
    }

    fn tick(&self) {
        let now = Instant::now();
        self.update(|t| t.tick(now, &self.counters));
    }

    fn on_update(&self, u: Update) {
        if let Update::Unlocked(p) = &u
            && let Some(r) = self.request.borrow_mut().as_mut()
        {
            r.set_password(p.clone());
        }
        // One last read so a pause shows exactly where it stopped.
        self.tick();
        self.update(|t| t.apply(u));
        if !self.running() {
            self.timer.stop();
            if self.quit_when_stopped.get() {
                let _ = slint::quit_event_loop();
            }
        }
    }

    fn update(&self, f: impl FnOnce(&mut Task)) {
        let mut task = self.task.borrow_mut();
        if let Some(t) = task.as_mut() {
            f(t);
            (self.show)(t);
        }
    }
}
