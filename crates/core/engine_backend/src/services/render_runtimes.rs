//! The engine's render runtimes: every Helio renderer the engine runs, owned
//! here and drawn by one scheduler.
//!
//! Helio is a library: it builds a renderer, runs a pipeline and draws a frame
//! into a view, and knows no thread, task or plugin. The engine owns the
//! renderers. Whoever needs one (the level editor's viewports, a plugin's
//! preview, a thumbnail) asks the service for a **runtime**
//! ([`RenderRuntimes::create`]) and gets a [`RuntimeHandle`]: the controls
//! (pause, resume, stop, update mode, visibility, a frame on demand) and the
//! renderer itself, for what the owner drives (camera, selection, its
//! mailbox). The runtime's frames go to its **output** ([`FrameTarget`]): a
//! UI triple buffer (the editor's `ui_helio_viewport::SurfaceTarget`), or an
//! offscreen texture with an async readback ([`OffscreenTarget`]).
//!
//! **One scheduler.** All runtimes are drawn by one service thread, "Helio
//! runtimes" (a persistent task with a heartbeat), in priority order, each at
//! its own pace, on the one device the outputs share. Unreal draws every
//! viewport from its one render thread and Godot's `RenderingServer` draws
//! all viewports in order each frame; the copied per-viewport loops this
//! replaces contended for the same queue and submit lock with no parallelism
//! to gain. The scheduler never waits for a consumer: an output whose last
//! frame was not taken yet ([`FrameTarget::ready`]) is skipped this pass, so a
//! hidden or slow view costs nothing. A renderer's first build (seconds: it
//! compiles pipelines) runs as a pooled task, so the other runtimes keep
//! drawing meanwhile.
//!
//! **States** ([`RuntimeState`]): starting (the first build), running, paused
//! (keeps its GPU resources, draws nothing), stopped (its renderer's GPU
//! resources are freed, the description kept; resuming builds again), lost
//! (its output's device was lost; it builds again on the next output) and
//! failed. **Update modes** ([`UpdateMode`]), as Godot's viewports have them:
//! always, when visible, on demand for N frames, disabled.
//!
//! **Visible.** Each runtime is a persistent task in the editor's Tasks system
//! (`editor_task_queue`): its log (what its renderer logs while it draws), its
//! phase (the state), its heartbeat (one per frame drawn) and its details
//! (owner, scene, size, mode, frame time), which the Tasks window's Renderers
//! view lists. The task system is GPUI-free, so a game build has the same.
//!
//! **Owners.** A runtime carries the origin of the code that asked for it
//! (`pulsar_plugins::origin!()`); when the plugin set changes, the runtimes
//! of a plugin that is no longer enabled are destroyed. Dropping the last
//! handle destroys a runtime; [`RenderRuntimes::shutdown`] stops the scheduler
//! and joins it.

mod device;
mod driven;
mod offscreen;
mod pacing;
mod runtime;
mod scheduler;
mod service;
#[cfg(test)]
mod tests;
mod types;

pub use device::request_engine_device;
pub use driven::{DrivenFrame, DrivenRuntime};
pub use offscreen::OffscreenTarget;
pub use pacing::{FrameKind, Pace, PacingPolicy, Presence};
pub use runtime::RuntimeHandle;
pub use service::{global, RenderRuntimes};
pub use types::{FrameTarget, RuntimeDescription, RuntimeState, SharedRenderer, UpdateMode};

/// Who a runtime's origin is, as the Renderers view names it.
fn owner_label(origin: &'static str) -> String {
    match pulsar_plugins::owner_of(origin) {
        pulsar_plugins::Owner::Plugin(plugin) => plugin.id.clone(),
        pulsar_plugins::Owner::Engine(name) => format!("engine ({name})"),
        pulsar_plugins::Owner::Unknown(origin) => origin.to_string(),
    }
}

/// The Tasks category of every runtime (the Renderers view lists it).
pub const RUNTIME_CATEGORY: &str = editor_task_queue::categories::RENDER_RUNTIME;
