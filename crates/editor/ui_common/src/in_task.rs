//! Off the UI thread, then back: the one way a view does non-UI work.
//!
//! The UI thread builds and updates views. Reading or writing files, querying
//! the file index, walking a folder, parsing, spawning a process: each runs in
//! an editor task on the pool, and its result comes back to the view on the
//! UI thread. [`InTask::in_task`] is that round trip:
//!
//! ```ignore
//! use ui_common::in_task::InTask as _;
//!
//! self.loading = Some(cx.in_task(
//!     TaskDescription::new("Read level list", "levels", TaskDuration::Short),
//!     move |ctx| {
//!         ctx.set_phase("Listing levels");
//!         list_levels(&root)
//!     },
//!     |panel, levels, cx| {
//!         panel.levels = levels;
//!         cx.notify();
//!     },
//! ));
//! ```
//!
//! The work shows in the Tasks window with its log; the view never waits.
//! Dropping the returned [`gpui::Task`] skips `apply` (the work itself still
//! finishes, since it may already be running). A blocking call made on the UI
//! thread instead is reported (`editor_task_queue::blocking_call`).

use editor_task_queue::{TaskContext, TaskDescription};
use gpui::{Context, Task};

/// Run work in an editor task and apply its result to the view.
pub trait InTask<V: 'static> {
    /// Run `work` as a pooled editor task, off the UI thread, then call
    /// `apply` with its result on the UI thread, if the view still exists
    /// and the returned task was not dropped.
    /// The task's origin in the Tasks window is the caller.
    #[track_caller]
    fn in_task<R: Send + 'static>(
        &mut self,
        description: TaskDescription,
        work: impl FnOnce(&TaskContext) -> R + Send + 'static,
        apply: impl FnOnce(&mut V, R, &mut Context<V>) + 'static,
    ) -> Task<()>;
}

impl<V: 'static> InTask<V> for Context<'_, V> {
    #[track_caller]
    fn in_task<R: Send + 'static>(
        &mut self,
        description: TaskDescription,
        work: impl FnOnce(&TaskContext) -> R + Send + 'static,
        apply: impl FnOnce(&mut V, R, &mut Context<V>) + 'static,
    ) -> Task<()> {
        let (tx, rx) = smol::channel::bounded(1);
        editor_task_queue::global().submit(description, move |ctx| {
            let _ = tx.send_blocking(work(&ctx));
            Ok::<(), String>(())
        });
        self.spawn(async move |view, cx| {
            // A closed channel means the task failed; its log says why.
            let Ok(result) = rx.recv().await else {
                return;
            };
            let _ = view.update(cx, |view, cx| apply(view, result, cx));
        })
    }
}
