//! Run a closure on the UI thread from any other thread, and wait for its result.
//!
//! The reverse of [`crate::in_task`]: that moves work off the UI thread and brings
//! the result back; this lets work that is already off it (an MCP request on a
//! server worker) do the one thing that must happen on the UI thread (start a
//! build with its window and notifications, move a viewport camera) and read
//! what it returned.
//!
//! The editor shell calls [`install`] once at startup. From then on [`call`]
//! queues the closure for the UI thread's executor and blocks the calling
//! thread, never the UI thread: calling it *from* the UI thread is refused (it
//! would wait for itself).

use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

use gpui::{App, AsyncApp};

type Job = Box<dyn FnOnce(&mut App) + Send>;

fn queue() -> &'static OnceLock<smol::channel::Sender<Job>> {
    static QUEUE: OnceLock<smol::channel::Sender<Job>> = OnceLock::new();
    &QUEUE
}

/// Start the bridge: a persistent editor task on the UI thread's executor takes
/// queued closures and runs each with the app. Calling it twice does nothing.
pub fn install(cx: &mut App) {
    let (send, receive) = smol::channel::unbounded::<Job>();
    if queue().set(send).is_err() {
        return;
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        let _task = editor_task_queue::global().persistent_task(
            editor_task_queue::TaskDescription::new(
                "UI thread bridge for tool calls",
                "ui",
                editor_task_queue::TaskDuration::Long,
            ),
            None,
        );
        while let Ok(job) = receive.recv().await {
            let _ = cx.update(|app| job(app));
        }
    })
    .detach();
}

/// Run `work` on the UI thread and return what it returned. `Err` when the
/// bridge is not installed, the UI thread did not get to it within `timeout`,
/// or the caller is the UI thread.
pub fn call<R: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce(&mut App) -> R + Send + 'static,
) -> Result<R, String> {
    if editor_task_queue::on_ui_thread() {
        return Err("ui_call::call was made on the UI thread; do the work directly".into());
    }
    let Some(sender) = queue().get() else {
        return Err("the editor's UI bridge is not running (no editor window yet)".into());
    };
    let (reply, wait) = mpsc::sync_channel(1);
    sender
        .send_blocking(Box::new(move |app: &mut App| {
            let _ = reply.send(work(app));
        }))
        .map_err(|_| "the editor's UI bridge has stopped".to_owned())?;
    wait.recv_timeout(timeout)
        .map_err(|_| format!("the UI thread did not run the request within {timeout:?}"))
}
