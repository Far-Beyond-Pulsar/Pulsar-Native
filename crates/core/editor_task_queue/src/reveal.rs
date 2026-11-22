//! A request to show one task: whatever opens the window that lists tasks
//! (the Tasks window) asks for a task first, and that window takes the request
//! when it next draws, selecting the task in the view that lists it.
//!
//! The asker names no window and the window no asker: a language server's
//! "Open Log" (in the shell) reaches its task this way without the shell
//! depending on the tool that draws tasks.

use std::sync::Mutex;

use crate::TaskId;

static REVEAL: Mutex<Option<TaskId>> = Mutex::new(None);

/// Ask whatever shows tasks to show `id` (the latest request wins).
pub fn request_reveal(id: TaskId) {
    *REVEAL.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(id);
}

/// The task to show, if one was asked for since the last call.
pub fn take_reveal_request() -> Option<TaskId> {
    REVEAL
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_request_is_taken_once() {
        let task = |title: &str| {
            crate::global().persistent_task(crate::TaskDescription::persistent(title, "test"), None)
        };
        let (first, second) = (task("reveal test a"), task("reveal test b"));
        let (first, second) = (first.context().id(), second.context().id());
        request_reveal(first);
        request_reveal(second);
        assert_eq!(take_reveal_request(), Some(second));
        assert_eq!(take_reveal_request(), None);
    }
}
