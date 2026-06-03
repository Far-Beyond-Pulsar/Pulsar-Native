//! Multi-window management for Pulsar game runtime.
//!
//! Game code interacts with windows through [`WindowManager`], a resource that
//! lives in the ECS [`World`] and can be accessed from any actor or system.
//! The actual GPU/winit state lives on the main thread inside
//! [`crate::windowed_app::PulsarApp`]; the two sides communicate through
//! [`WindowBridge`].
//!
//! # Window lifecycle
//!
//! ```rust,ignore
//! // In begin_play or a system:
//! let wm = world.resource::<WindowManager>();
//! let handle = wm.open(WindowDescriptor {
//!     title: "My Window".into(),
//!     width: 1280,
//!     height: 720,
//! });
//!
//! // Update the camera every tick:
//! wm.set_camera(handle, RenderCamera {
//!     position: [0.0, 2.0, 8.0],
//!     target:   [0.0, 0.0, 0.0],
//!     up:       [0.0, 1.0, 0.0],
//!     fov_y:    std::f32::consts::FRAC_PI_4,
//!     near:     0.1,
//!     far:      1000.0,
//! });
//!
//! // Close when done:
//! wm.close(handle);
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use winit::event_loop::EventLoopProxy;

// ── Public types ──────────────────────────────────────────────────────────────

/// Opaque handle identifying a game window.
///
/// Returned by [`WindowManager::open`]. Stable until [`WindowManager::close`]
/// is called. Does **not** map 1:1 to winit's `WindowId`; the mapping is
/// maintained inside [`PulsarApp`][crate::windowed_app::PulsarApp].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WindowHandle(u64);

static NEXT_WINDOW_HANDLE: AtomicU64 = AtomicU64::new(1);

impl WindowHandle {
    pub(crate) fn next() -> Self {
        Self(NEXT_WINDOW_HANDLE.fetch_add(1, Ordering::Relaxed))
    }

    /// The numeric ID, for logging.
    pub fn id(self) -> u64 {
        self.0
    }
}

/// Parameters for opening a new window.
#[derive(Clone, Debug)]
pub struct WindowDescriptor {
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// Whether to start in editor mode (helio grid / gizmos enabled).
    pub editor_mode: bool,
}

impl Default for WindowDescriptor {
    fn default() -> Self {
        Self {
            title: "Pulsar".into(),
            width: 1280,
            height: 720,
            editor_mode: false,
        }
    }
}

/// A camera description pushed from game code into the render thread.
///
/// Aspect ratio is computed automatically from the window dimensions at render
/// time, so you never need to update the camera just because a window resized.
#[derive(Clone, Debug)]
pub struct RenderCamera {
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    /// Vertical field of view in radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for RenderCamera {
    fn default() -> Self {
        Self {
            position: [0.0, 2.0, 8.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_4,
            near: 0.1,
            far: 1000.0,
        }
    }
}

// ── Inter-thread commands ──────────────────────────────────────────────────────

/// Commands sent from the ECS thread to the main (winit) thread.
///
/// Delivered via `EventLoopProxy<WindowCommand>`, which wakes the event loop
/// immediately — no polling required.
#[derive(Debug)]
pub enum WindowCommand {
    /// Open a new window with this handle and descriptor.
    Open {
        handle: WindowHandle,
        desc: WindowDescriptor,
    },
    /// Destroy the window identified by this handle.
    Close { handle: WindowHandle },
}

// ── Bridge ────────────────────────────────────────────────────────────────────

/// Shared state between the ECS thread and the main render thread.
///
/// - Open/close commands flow through the winit `EventLoopProxy` (zero-latency wakeup).
/// - Per-frame camera updates are stored in a `Mutex<HashMap<…>>` and read
///   each frame by the render thread; a write never blocks longer than a
///   `HashMap` insert.
pub struct WindowBridge {
    /// Sends `WindowCommand`s to the main thread via winit's event loop.
    proxy: EventLoopProxy<WindowCommand>,
    /// Latest camera per window. Render thread reads; ECS thread writes.
    cameras: Mutex<HashMap<WindowHandle, RenderCamera>>,
}

impl WindowBridge {
    pub(crate) fn new(proxy: EventLoopProxy<WindowCommand>) -> Self {
        Self {
            proxy,
            cameras: Mutex::new(HashMap::new()),
        }
    }

    /// Push a command to the main thread (non-blocking).
    pub(crate) fn send(&self, cmd: WindowCommand) {
        // Errors only if the event loop has already shut down; safe to ignore.
        let _ = self.proxy.send_event(cmd);
    }

    /// Write a camera update (called from ECS thread).
    pub(crate) fn set_camera(&self, handle: WindowHandle, camera: RenderCamera) {
        if let Ok(mut map) = self.cameras.lock() {
            map.insert(handle, camera);
        }
    }

    /// Read the latest camera for a window (called from main thread each frame).
    pub fn camera(&self, handle: WindowHandle) -> Option<RenderCamera> {
        self.cameras.lock().ok()?.get(&handle).cloned()
    }

    /// Remove camera entry when a window closes (called from main thread).
    pub(crate) fn remove_camera(&self, handle: WindowHandle) {
        if let Ok(mut map) = self.cameras.lock() {
            map.remove(&handle);
        }
    }
}

// ── WindowManager (ECS resource) ──────────────────────────────────────────────

/// ECS resource that game code uses to manage windows.
///
/// Add this to your [`World`][crate::World] via
/// [`TickLoop::run_with_windows`][crate::TickLoop::run_with_windows], which
/// injects it automatically before starting the ECS tick thread.
///
/// Access it from a system:
/// ```rust,ignore
/// fn my_system(world: &mut World) {
///     let wm = world.resource::<WindowManager>().clone();
///     let handle = wm.open(WindowDescriptor::default());
/// }
/// ```
#[derive(Clone)]
pub struct WindowManager {
    bridge: Arc<WindowBridge>,
}

impl WindowManager {
    pub(crate) fn new(bridge: Arc<WindowBridge>) -> Self {
        Self { bridge }
    }

    /// Request a new window.  Returns a [`WindowHandle`] immediately; the
    /// window opens asynchronously on the main thread (usually within one
    /// event-loop iteration, i.e. < 16 ms).
    pub fn open(&self, desc: WindowDescriptor) -> WindowHandle {
        let handle = WindowHandle::next();
        self.bridge.send(WindowCommand::Open { handle, desc });
        handle
    }

    /// Request that a window be closed and its GPU resources released.
    pub fn close(&self, handle: WindowHandle) {
        self.bridge.remove_camera(handle);
        self.bridge.send(WindowCommand::Close { handle });
    }

    /// Push a new camera for the given window.  The render thread picks it up
    /// before the next frame is drawn.  Safe to call every tick.
    pub fn set_camera(&self, handle: WindowHandle, camera: RenderCamera) {
        self.bridge.set_camera(handle, camera);
    }

    /// Access the raw bridge (for advanced use — prefer the helpers above).
    pub fn bridge(&self) -> &Arc<WindowBridge> {
        &self.bridge
    }
}
