//! One runtime's shared state and the handle its owner controls it with.
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    global, owner_label, FrameTarget, RenderRuntimes, RuntimeDescription, RuntimeState,
    SharedRenderer, UpdateMode,
};

pub(super) struct Runtime {
    pub(super) id: u64,
    pub(super) description: Mutex<RuntimeDescription>,
    pub(super) renderer: SharedRenderer,
    pub(super) output: Mutex<Option<Arc<dyn FrameTarget>>>,
    pub(super) state: Mutex<RuntimeState>,
    /// The owner asked to pause or stop it (cleared by resume).
    pub(super) held: Mutex<Option<RuntimeState>>,
    pub(super) visible: AtomicBool,
    /// Where its window stands ([`super::Presence`] bits): focused, minimized.
    pub(super) presence: AtomicU8,
    /// What its last frame was for ([`super::FrameKind`] bits).
    pub(super) kind: AtomicU8,
    /// Frames still owed in [`UpdateMode::OnDemand`].
    pub(super) demanded: AtomicU32,
    /// Called right before each frame (the owner samples its input late).
    pub(super) before_frame: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// The next frame starts its temporal history over.
    pub(super) reset_history: AtomicBool,
    /// A first build is running on the task pool.
    pub(super) building: AtomicBool,
    /// Running but drawing nothing on purpose (hidden, no frames asked for):
    /// its task is idle, so the missing heartbeat is no stall.
    pub(super) quiet: AtomicBool,
    pub(super) last_frame: Mutex<Option<Instant>>,
    /// The last frame's CPU time, in microseconds.
    pub(super) frame_us: AtomicU64,
    pub(super) frames: AtomicU64,
    pub(super) task: editor_task_queue::PersistentTask,
}

impl Runtime {
    pub(super) fn set_state(&self, state: RuntimeState) {
        let mut current = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *current == state {
            return;
        }
        self.task.context().set_phase(state.label());
        let idle = !matches!(state, RuntimeState::Running | RuntimeState::Starting);
        self.task.context().set_idle(idle);
        self.quiet.store(idle, Ordering::Release);
        *current = state;
    }

    /// Running and not drawing on purpose (`true`), or drawing again.
    pub(super) fn set_quiet(&self, quiet: bool) {
        if self.quiet.swap(quiet, Ordering::AcqRel) != quiet {
            self.task.context().set_idle(quiet);
        }
    }

    pub(super) fn state(&self) -> RuntimeState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(super) fn publish_details(&self) {
        let description = self.description.lock().unwrap_or_else(|e| e.into_inner());
        let size = self
            .output
            .lock()
            .ok()
            .and_then(|output| output.as_ref().map(|output| output.size()))
            .map_or_else(|| "no output".to_string(), |(w, h)| format!("{w}x{h}"));
        let owner = owner_label(description.origin);
        let frame_ms = self.frame_us.load(Ordering::Relaxed) as f64 / 1000.0;
        // The graph's textures (a renderer that is drawing holds its lock:
        // the last value stays until the next publish).
        let graph_mb = self
            .renderer
            .try_lock()
            .ok()
            .and_then(|renderer| renderer.graph_texture_bytes())
            .map_or_else(
                || "-".to_string(),
                |bytes| format!("{:.0} MB", bytes as f64 / 1_048_576.0),
            );
        self.task.context().set_details(vec![
            ("Owner".into(), owner),
            ("Scene".into(), description.scene_label.clone()),
            ("Size".into(), size),
            ("Mode".into(), description.mode.label()),
            ("State".into(), self.state().label()),
            (
                "Visible".into(),
                self.visible.load(Ordering::Relaxed).to_string(),
            ),
            ("Window".into(), {
                let presence = self.presence();
                if presence.minimized {
                    "minimized".to_string()
                } else if presence.focused {
                    "focused".to_string()
                } else {
                    "in the background".to_string()
                }
            }),
            ("Last frame".into(), format!("{:?}", self.frame_kind())),
            ("Frame (CPU)".into(), format!("{frame_ms:.2} ms")),
            ("Graph textures".into(), graph_mb),
            (
                "Frames".into(),
                self.frames.load(Ordering::Relaxed).to_string(),
            ),
        ]);
    }

    /// Whether it should draw now, and when it is next due otherwise.
    pub(super) fn due(&self, now: Instant, interval: Duration) -> Result<(), Instant> {
        let last = *self.last_frame.lock().unwrap_or_else(|e| e.into_inner());
        match last {
            Some(last) if now < last + interval => Err(last + interval),
            _ => Ok(()),
        }
    }

    pub(super) fn presence(&self) -> super::Presence {
        super::Presence::from_bits(self.presence.load(Ordering::Acquire))
    }

    pub(super) fn frame_kind(&self) -> super::FrameKind {
        super::FrameKind::from_bits(self.kind.load(Ordering::Acquire))
    }

    pub(super) fn wants_frames(&self) -> bool {
        let mode = self
            .description
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode;
        match mode {
            UpdateMode::Always => true,
            UpdateMode::WhenVisible => self.visible.load(Ordering::Acquire),
            UpdateMode::OnDemand(_) => self.demanded.load(Ordering::Acquire) > 0,
            UpdateMode::Disabled => false,
        }
    }
}

/// The controls of one runtime, for its owner. Cheap to clone; when the last
/// clone drops, the runtime is destroyed (its renderer is freed once a frame
/// in flight finished).
#[derive(Clone)]
pub struct RuntimeHandle {
    pub(super) runtime: Arc<Runtime>,
    pub(super) _owner: Arc<Owned>,
}

/// Destroys its runtime when the last handle goes.
pub(super) struct Owned {
    pub(super) id: u64,
    pub(super) service: &'static RenderRuntimes,
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.service.destroy(self.id);
    }
}

impl RuntimeHandle {
    pub fn id(&self) -> u64 {
        self.runtime.id
    }

    /// The renderer, for what the owner drives (camera, selection, mailbox).
    pub fn renderer(&self) -> &SharedRenderer {
        &self.runtime.renderer
    }

    pub fn state(&self) -> RuntimeState {
        self.runtime.state()
    }

    /// Whether its renderer is being built (a first build, a pipeline swap,
    /// changed graph settings): it shows its last frame meanwhile.
    pub fn is_building(&self) -> bool {
        self.runtime.building.load(Ordering::Acquire)
    }

    /// Frames drawn so far.
    pub fn frames(&self) -> u64 {
        self.runtime.frames.load(Ordering::Acquire)
    }

    /// Draw into `output` from now on (a new surface after a resize of the
    /// window's device, a lost device's replacement).
    pub fn set_output(&self, output: Arc<dyn FrameTarget>) {
        // Its consumer taking a frame is the scheduler's cue to draw the next
        // (rather than polling `ready` a quarter frame later).
        output.set_ready_waker(Some(Arc::new(|| global().wake())));
        *self
            .runtime
            .output
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(output);
        // A new output is a new start for one that lost its device or could
        // not build on the old one.
        if matches!(self.state(), RuntimeState::Lost | RuntimeState::Failed(_)) {
            self.runtime.set_state(RuntimeState::Starting);
        }
        self.runtime.publish_details();
        global().wake();
    }

    /// Stop drawing; keep the renderer and its GPU resources.
    pub fn pause(&self) {
        self.hold(RuntimeState::Paused);
    }

    /// Stop drawing and free the renderer's GPU resources; the description
    /// stays, and [`resume`](Self::resume) builds them again.
    pub fn stop(&self) {
        self.hold(RuntimeState::Stopped);
    }

    /// Draw again after [`pause`](Self::pause) or [`stop`](Self::stop).
    pub fn resume(&self) {
        *self.runtime.held.lock().unwrap_or_else(|e| e.into_inner()) = None;
        global().wake();
    }

    fn hold(&self, state: RuntimeState) {
        *self.runtime.held.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        global().wake();
    }

    pub fn set_mode(&self, mode: UpdateMode) {
        if let UpdateMode::OnDemand(n) = mode {
            self.runtime.demanded.store(n, Ordering::Release);
        }
        self.runtime
            .description
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode = mode;
        self.runtime.publish_details();
        global().wake();
    }

    /// Ask for `n` more frames (an [`UpdateMode::OnDemand`] runtime).
    pub fn request_frames(&self, n: u32) {
        self.runtime.demanded.fetch_add(n, Ordering::AcqRel);
        global().wake();
    }

    /// Call `callback` right before each frame, on the scheduler's thread
    /// (sample input as late as possible).
    pub fn set_before_frame(&self, callback: Box<dyn Fn() + Send + Sync>) {
        *self
            .runtime
            .before_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(callback);
    }

    /// The next frame starts its temporal history over (the view was hidden
    /// and the camera moved meanwhile).
    pub fn reset_history(&self) {
        self.runtime.reset_history.store(true, Ordering::Release);
    }

    /// Where the window showing its output stands (focused, minimized): the
    /// service's frame policy ([`super::PacingPolicy`]) draws a visible
    /// viewport of a background window at a low rate and a minimized one
    /// not at all. A change wakes the scheduler, so a window that is focused
    /// again draws on its next pass with nothing rebuilt.
    pub fn set_presence(&self, presence: super::Presence) {
        if self
            .runtime
            .presence
            .swap(presence.to_bits(), Ordering::AcqRel)
            != presence.to_bits()
        {
            self.runtime.publish_details();
            global().wake();
        }
    }

    /// Whether its output is shown (a tab in front); a
    /// [`UpdateMode::WhenVisible`] runtime draws only then.
    pub fn set_visible(&self, visible: bool) {
        if self.runtime.visible.swap(visible, Ordering::AcqRel) != visible {
            self.runtime.publish_details();
            global().wake();
        }
    }
}
