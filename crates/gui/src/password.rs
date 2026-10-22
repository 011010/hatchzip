//! The password prompt. The engine asks from a worker thread, in the middle
//! of opening an archive; the question crosses to the UI thread, and the
//! worker waits on a channel until the person answers or gives up.
//!
//! Passwords live in memory for this process only. A run resumed later, in
//! a new process, asks again.

use std::cell::RefCell;
use std::sync::mpsc::{Sender, channel};

pub struct Question {
    pub name: String,
    /// The last password didn't open it.
    pub wrong: bool,
    reply: Sender<Option<String>>,
}

impl Question {
    pub fn answer(self, password: Option<String>) {
        let _ = self.reply.send(password);
    }
}

type Host = Box<dyn Fn(Question)>;

thread_local! {
    // Whichever window is open shows the prompt. Without one, the question
    // is dropped, which the worker reads as giving up.
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Called on the UI thread by the window that will show the prompt.
pub fn set_host(show: impl Fn(Question) + 'static) {
    HOST.with(|h| *h.borrow_mut() = Some(Box::new(show)));
}

/// Asks on the UI thread and waits for the answer. For worker threads only:
/// called on the UI thread it would wait forever.
pub fn ask(name: &str, wrong: bool) -> Option<String> {
    let (reply, answer) = channel();
    let question = Question {
        name: name.to_string(),
        wrong,
        reply,
    };
    slint::invoke_from_event_loop(move || {
        HOST.with(|h| {
            if let Some(show) = &*h.borrow() {
                show(question);
            }
        })
    })
    .ok()?;
    answer.recv().ok().flatten()
}

/// The window side: holds the open question until the person answers.
#[derive(Default)]
pub struct Pending(RefCell<Option<Question>>);

impl Pending {
    /// Takes a new question. One that was still open is given up on; the
    /// engine never asks two at once, so that only happens on a stale one.
    pub fn put(&self, q: Question) {
        if let Some(old) = self.0.replace(Some(q)) {
            old.answer(None);
        }
    }

    pub fn answer(&self, password: Option<String>) {
        if let Some(q) = self.0.take() {
            q.answer(password);
        }
    }
}

/// Shows questions in a window with the `password-*` properties and
/// callbacks that host a `PasswordPrompt`. A macro because the main and task
/// windows are different generated types with the same members.
macro_rules! host_in {
    ($ui:expr) => {{
        let pending = std::rc::Rc::new($crate::password::Pending::default());
        let (p, weak) = (pending.clone(), $ui.as_weak());
        $crate::password::set_host(move |q| {
            if let Some(ui) = weak.upgrade() {
                ui.set_password_name(q.name.as_str().into());
                ui.set_password_wrong(q.wrong);
                ui.set_password_shown(true);
                p.put(q);
            }
        });
        let (p, weak) = (pending.clone(), $ui.as_weak());
        $ui.on_password_ok(move |text| {
            if let Some(ui) = weak.upgrade() {
                ui.set_password_shown(false);
            }
            p.answer(Some(text.to_string()));
        });
        let (p, weak) = (pending, $ui.as_weak());
        $ui.on_password_cancel(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_password_shown(false);
            }
            p.answer(None);
        });
    }};
}
pub(crate) use host_in;
