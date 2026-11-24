//! What a runtime is and what it draws into: update modes, states, the output
//! trait and the description its owner gives.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::services::gpu_renderer::GpuRenderer;

/// How often a runtime draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMode {
    /// Every frame, visible or not (a capture, a game view that must advance).
    Always,
    /// Every frame while its output is shown; nothing while hidden.
    WhenVisible,
    /// The next `n` frames, then nothing until asked again
    /// ([`RuntimeHandle::request_frames`]).
    OnDemand(u32),
    /// Never.
    Disabled,
}

impl UpdateMode {
    pub(super) fn label(self) -> String {
        match self {
            Self::Always => "Always".into(),
            Self::WhenVisible => "When visible".into(),
            Self::OnDemand(n) => format!("On demand ({n} left)"),
            Self::Disabled => "Disabled".into(),
        }
    }
}

/// Where a runtime is in its life.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    /// Waiting for an output, or building its renderer (pipelines compile).
    Starting,
    /// Drawing as its update mode says.
    Running,
    /// Not drawing; its renderer and GPU resources are kept.
    Paused,
    /// Not drawing; its renderer's GPU resources were freed. Resuming builds
    /// them again from the same description.
    Stopped,
    /// Its output's device was lost; it builds again on the next output.
    Lost,
    /// Its renderer could not be built.
    Failed(String),
}

impl RuntimeState {
    pub(super) fn label(&self) -> String {
        match self {
            Self::Starting => "Starting".into(),
            Self::Running => "Running".into(),
            Self::Paused => "Paused".into(),
            Self::Stopped => "Stopped".into(),
            Self::Lost => "Device lost".into(),
            Self::Failed(reason) => format!("Failed: {reason}"),
        }
    }
}

/// What a runtime draws into. Implemented by each kind of output: a UI
/// triple buffer (the editor's), an offscreen texture.
pub trait FrameTarget: Send + Sync + 'static {
    /// The device and queue frames for it are drawn on.
    fn device(&self) -> wgpu::Device;
    fn queue(&self) -> wgpu::Queue;
    fn format(&self) -> wgpu::TextureFormat;
    /// Its size in pixels; `(0, 0)` while it has none.
    fn size(&self) -> (u32, u32);
    /// Whether its device was lost.
    fn is_lost(&self) -> bool {
        false
    }
    /// Whether it can take a frame now (its consumer took the last one).
    fn ready(&self) -> bool {
        true
    }
    /// Call `waker` when it becomes [`ready`](Self::ready) again (its
    /// consumer took the last frame); `None` stops. An output that never
    /// waits needs none. The waker only signals; it never blocks.
    fn set_ready_waker(&self, _waker: Option<Arc<dyn Fn() + Send + Sync>>) {}
    /// How long one frame lasts at its consumer's pace (a display's refresh).
    fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / 60.0)
    }
    /// Draw one frame: `draw` records into the view it is given (with its
    /// size) and returns its submission; the target then publishes it.
    /// Returns whether a frame was published.
    fn draw(
        &self,
        draw: &mut dyn FnMut(&wgpu::TextureView, u32, u32) -> Option<wgpu::SubmissionIndex>,
    ) -> bool;
}

/// What a runtime is: who asked for it and how it runs.
pub struct RuntimeDescription {
    /// Shown in the Tasks window ("Level viewport", "Render graph preview").
    pub label: String,
    /// The code that asked for it: `pulsar_plugins::origin!()`.
    pub origin: &'static str,
    /// The scene it draws, as the Renderers view names it.
    pub scene_label: String,
    /// Higher draws first in a pass.
    pub priority: i32,
    pub mode: UpdateMode,
}

/// A runtime's renderer: the editor-style Helio renderer (a scene, a camera,
/// its mailbox), shared with the owner.
pub type SharedRenderer = Arc<Mutex<GpuRenderer>>;
