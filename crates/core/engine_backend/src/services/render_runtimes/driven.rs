//! Host-driven runtimes: a game's own loop draws, the service lists it.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{owner_label, RuntimeDescription, RuntimeState, RUNTIME_CATEGORY};

/// A runtime whose frames its host draws on its own loop: a game, whose
/// frame is the step of its tick (simulation, then the frame of what it
/// simulated), standalone on the window's event loop or embedded in the
/// editor (Play in Editor). The service lists it like any runtime (a
/// persistent task in the Renderers view, with its frame time) and the
/// controls reach it: a paused one draws nothing
/// ([`DrivenRuntime::begin_frame`] says so).
pub struct DrivenRuntime {
    description: Mutex<RuntimeDescription>,
    /// Width in the high half, height in the low; 0 until set.
    size: AtomicU64,
    paused: AtomicBool,
    frames: AtomicU64,
    task: editor_task_queue::PersistentTask,
}

/// One frame of a [`DrivenRuntime`]: its time is recorded when it drops.
pub struct DrivenFrame<'a> {
    runtime: &'a DrivenRuntime,
    started: Instant,
}

impl Drop for DrivenFrame<'_> {
    fn drop(&mut self) {
        let took = self.started.elapsed();
        let frames = self.runtime.frames.fetch_add(1, Ordering::AcqRel) + 1;
        {
            profiling::profile_scope!("DrivenFrame: task tick");
            self.runtime.task.context().tick(took);
        }
        if frames % 30 == 1 {
            profiling::profile_scope!("DrivenFrame: publish details");
            self.runtime.publish_details(Some(took));
        }
    }
}

impl DrivenRuntime {
    /// Registers a host-driven runtime.
    #[track_caller]
    pub fn register(description: RuntimeDescription) -> Self {
        let task = editor_task_queue::global().persistent_task(
            editor_task_queue::TaskDescription::persistent(
                format!("Helio runtime: {}", description.label),
                RUNTIME_CATEGORY,
            ),
            None,
        );
        task.context().set_phase(RuntimeState::Running.label());
        let runtime = Self {
            description: Mutex::new(description),
            size: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            task,
        };
        runtime.publish_details(None);
        runtime
    }

    /// The host starts a frame: `None` while the runtime is paused (draw
    /// nothing). Keep the guard for the frame's work; its time is recorded.
    pub fn begin_frame(&self) -> Option<DrivenFrame<'_>> {
        if self.paused.load(Ordering::Acquire) {
            return None;
        }
        // The frame's logs belong to the runtime's task.
        Some(DrivenFrame {
            runtime: self,
            started: Instant::now(),
        })
    }

    /// The runtime's task, to enter its span around the frame's work.
    pub fn task(&self) -> &editor_task_queue::TaskContext {
        self.task.context()
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
        self.task.context().set_phase(RuntimeState::Paused.label());
        self.task.context().set_idle(true);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
        self.task.context().set_phase(RuntimeState::Running.label());
        self.task.context().set_idle(false);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Its size, for the Renderers view.
    pub fn set_size(&self, width: u32, height: u32) {
        self.size.store(
            u64::from(width) << 32 | u64::from(height),
            Ordering::Release,
        );
        self.publish_details(None);
    }

    fn publish_details(&self, frame: Option<Duration>) {
        let description = self.description.lock().unwrap_or_else(|e| e.into_inner());
        let size = self.size.load(Ordering::Acquire);
        let mut details = vec![
            ("Owner".into(), owner_label(description.origin)),
            ("Scene".into(), description.scene_label.clone()),
            (
                "Size".into(),
                if size == 0 {
                    "unknown".into()
                } else {
                    format!("{}x{}", size >> 32, size & 0xffff_ffff)
                },
            ),
            ("Mode".into(), "Driven by its host".into()),
            (
                "State".into(),
                if self.is_paused() {
                    "Paused"
                } else {
                    "Running"
                }
                .into(),
            ),
            (
                "Frames".into(),
                self.frames.load(Ordering::Relaxed).to_string(),
            ),
        ];
        if let Some(frame) = frame {
            details.push((
                "Frame (CPU)".into(),
                format!("{:.2} ms", frame.as_secs_f64() * 1000.0),
            ));
        }
        self.task.context().set_details(details);
    }
}
