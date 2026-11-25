//! The service: the registry of runtimes, the engine device, and the
//! scheduler thread's lifecycle.
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use super::device::headless_device;
use super::runtime::{Owned, Runtime};
use super::{
    RuntimeDescription, RuntimeHandle, RuntimeState, SharedRenderer, UpdateMode, RUNTIME_CATEGORY,
};

/// The service: every runtime and the scheduler that draws them.
pub struct RenderRuntimes {
    pub(super) runtimes: Mutex<Vec<Arc<Runtime>>>,
    next_id: AtomicU64,
    /// Set by anything that changes what the scheduler should do.
    pub(super) wake: (Mutex<bool>, Condvar),
    scheduler: Mutex<Option<editor_task_queue::ServiceHandle<()>>>,
    pub(super) stop: AtomicBool,
    device: Mutex<Option<(wgpu::Device, wgpu::Queue)>>,
    /// A renderer build is running. Builds go one at a time: each compiles
    /// many pipelines synchronously in the driver, and several at once (a
    /// split layout's viewports) only contend.
    pub(super) building: AtomicBool,
}

/// The process's service.
pub fn global() -> &'static RenderRuntimes {
    // Static: the editor, the game and headless tools all draw through it, and
    // a game has no editor context to hang it on.
    static SERVICE: OnceLock<RenderRuntimes> = OnceLock::new();
    SERVICE.get_or_init(|| RenderRuntimes {
        runtimes: Mutex::new(Vec::new()),
        next_id: AtomicU64::new(1),
        wake: (Mutex::new(false), Condvar::new()),
        scheduler: Mutex::new(None),
        stop: AtomicBool::new(false),
        device: Mutex::new(None),
        building: AtomicBool::new(false),
    })
}

impl RenderRuntimes {
    /// Use `device` for the engine's GPU work that has no output of its own
    /// (thumbnails, the render graph pin probe): the editor offers its
    /// window's, the game its own. Only the first offer counts.
    pub fn offer_device(&self, device: wgpu::Device, queue: wgpu::Queue) {
        let mut slot = self.device.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some((device, queue));
        }
    }

    /// The engine's device: the one offered, else one made for this process
    /// (headless: a tool or test with no window). `None` without a GPU.
    pub fn device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        let mut slot = self.device.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = headless_device();
        }
        slot.clone()
    }

    /// A new runtime drawing `renderer` (a fresh one when `None`) as
    /// `description` says. It starts once it has an output
    /// ([`RuntimeHandle::set_output`]).
    pub fn create(
        &'static self,
        description: RuntimeDescription,
        renderer: SharedRenderer,
    ) -> RuntimeHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let task = editor_task_queue::global().persistent_task(
            editor_task_queue::TaskDescription::persistent(
                format!("Helio runtime: {}", description.label),
                RUNTIME_CATEGORY,
            ),
            // A frame per pass at least every few seconds while running; idle
            // states turn the check off.
            Some(Duration::from_secs(5)),
        );
        let demanded = match description.mode {
            UpdateMode::OnDemand(n) => n,
            _ => 0,
        };
        let runtime = Arc::new(Runtime {
            id,
            description: Mutex::new(description),
            renderer,
            output: Mutex::new(None),
            state: Mutex::new(RuntimeState::Starting),
            held: Mutex::new(None),
            visible: AtomicBool::new(true),
            presence: std::sync::atomic::AtomicU8::new(super::Presence::default().to_bits()),
            kind: std::sync::atomic::AtomicU8::new(super::FrameKind::Interactive.to_bits()),
            demanded: AtomicU32::new(demanded),
            before_frame: Mutex::new(None),
            reset_history: AtomicBool::new(false),
            building: AtomicBool::new(false),
            quiet: AtomicBool::new(false),
            last_frame: Mutex::new(None),
            frame_us: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            task,
        });
        runtime
            .task
            .context()
            .set_phase(RuntimeState::Starting.label());
        runtime.task.context().set_idle(true);
        runtime.publish_details();
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::clone(&runtime));
        self.ensure_scheduler();
        self.wake();
        RuntimeHandle {
            runtime,
            _owner: Arc::new(Owned { id, service: self }),
        }
    }

    /// Every runtime's id, label and state, for tools and tests.
    pub fn list(&self) -> Vec<(u64, String, RuntimeState)> {
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|runtime| {
                let label = runtime
                    .description
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .label
                    .clone();
                (runtime.id, label, runtime.state())
            })
            .collect()
    }

    pub(super) fn destroy(&self, id: u64) {
        let removed = {
            let mut runtimes = self.runtimes.lock().unwrap_or_else(|e| e.into_inner());
            let at = runtimes.iter().position(|runtime| runtime.id == id);
            at.map(|at| runtimes.remove(at))
        };
        if let Some(runtime) = removed {
            runtime.task.context().info("destroyed");
            // Its task ends with its last handle, not when the scheduler,
            // mid-step, lets go of it.
            runtime.task.finish();
        }
        self.wake();
    }

    /// Stops the scheduler and waits for it; runtimes stop drawing.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        self.wake();
        let handle = self
            .scheduler
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    pub(super) fn wake(&self) {
        let (flag, condvar) = &self.wake;
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        condvar.notify_all();
    }

    fn ensure_scheduler(&'static self) {
        let mut slot = self.scheduler.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() || self.stop.load(Ordering::Acquire) {
            return;
        }
        let spawned = editor_task_queue::spawn_service(
            editor_task_queue::ServiceDescription::new(
                "Helio runtimes",
                "render",
                "Helio Runtimes",
                editor_task_queue::ThreadRole::Render,
            )
            .owner("engine")
            .heartbeat(Duration::from_secs(3)),
            move |ctx| self.run(ctx),
        );
        match spawned {
            Ok(handle) => *slot = Some(handle),
            Err(error) => tracing::error!("The Helio runtimes scheduler did not start: {error}"),
        }
    }
}
