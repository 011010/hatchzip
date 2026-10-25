//! The prompt that asks whether to replace files already in the destination
//! folder. Like the password prompt, the question comes from the worker
//! thread before anything is touched, crosses to the UI thread, and the
//! worker waits for the answer.

use std::cell::RefCell;
use std::path::Path;
use std::sync::mpsc::{Sender, channel};

pub struct Question {
    pub text: String,
    reply: Sender<bool>,
}

type Host = Box<dyn Fn(Question)>;

thread_local! {
    // Whichever window is open shows the prompt. Without one, the question
    // is dropped, which the worker reads as a no.
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Called on the UI thread by the window that will show the prompt.
pub fn set_host(show: impl Fn(Question) + 'static) {
    HOST.with(|h| *h.borrow_mut() = Some(Box::new(show)));
}

/// Asks on the UI thread whether to replace the `count` files in the way in
/// `folder`, and waits for the answer. For worker threads only.
pub fn ask(count: usize, folder: &Path) -> bool {
    let (reply, answer) = channel();
    let question = Question {
        text: crate::text::replace_question(count, folder),
        reply,
    };
    let sent = slint::invoke_from_event_loop(move || {
        HOST.with(|h| {
            if let Some(show) = &*h.borrow() {
                show(question);
            }
        })
    });
    sent.is_ok() && answer.recv().unwrap_or(false)
}

/// The window side: holds the open question until the person answers.
#[derive(Default)]
pub struct Pending(RefCell<Option<Question>>);

impl Pending {
    pub fn put(&self, q: Question) {
        if let Some(old) = self.0.replace(Some(q)) {
            let _ = old.reply.send(false);
        }
    }

    pub fn answer(&self, replace: bool) {
        if let Some(q) = self.0.take() {
            let _ = q.reply.send(replace);
        }
    }
}

/// Shows questions in a window with the `replace-*` properties and
/// callbacks that host a `ReplacePrompt`, as `password::host_in` does.
macro_rules! host_in {
    ($ui:expr) => {{
        let pending = std::rc::Rc::new($crate::replace::Pending::default());
        let (p, weak) = (pending.clone(), $ui.as_weak());
        $crate::replace::set_host(move |q| {
            if let Some(ui) = weak.upgrade() {
                ui.set_replace_text(q.text.as_str().into());
                ui.set_replace_shown(true);
                p.put(q);
            }
        });
        let (p, weak) = (pending.clone(), $ui.as_weak());
        $ui.on_replace_ok(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_replace_shown(false);
            }
            p.answer(true);
        });
        let (p, weak) = (pending, $ui.as_weak());
        $ui.on_replace_cancel(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_replace_shown(false);
            }
            p.answer(false);
        });
    }};
}
pub(crate) use host_in;
