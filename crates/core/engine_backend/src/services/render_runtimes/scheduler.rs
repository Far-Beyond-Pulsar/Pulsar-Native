//! The scheduler: one thread that draws every runtime in priority order, each
//! at its output's pace, and builds renderers on the task pool.
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::runtime::Runtime;
use super::{global, FrameTarget, RenderRuntimes, RuntimeState, UpdateMode};

/// Raises the system timer resolution to 1 ms while held.
///
/// The scheduler paces frames by sleeping on a condvar until the next is due.
/// On Windows that sleep wakes on the system timer tick, 15.625 ms by default:
/// every frame was held to the next tick, so the viewport drew at 64 Hz on any
/// display faster than that, with the render thread idle for a third of the
/// time (a captured profile: frames exactly 15.5 to 15.6 ms apart, 9.6 ms of
/// work in each). Unreal and Chromium raise it the same way. Since Windows 10
/// 2004 the request applies to this process only.
struct FineTimerResolution;

impl FineTimerResolution {
    const PERIOD_MS: u32 = 1;

    fn acquire() -> Self {
        #[cfg(windows)]
        // SAFETY: a plain winmm call; paired with `timeEndPeriod` in `drop`.
        unsafe {
            windows::Win32::Media::timeBeginPeriod(Self::PERIOD_MS);
        }
        Self
    }
}

impl Drop for FineTimerResolution {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: ends the period `acquire` began.
        unsafe {
            windows::Win32::Media::timeEndPeriod(Self::PERIOD_MS);
        }
    }
}

/// The scheduler's longest sleep when nothing is due.
const IDLE_WAIT: Duration = Duration::from_millis(250);
impl RenderRuntimes {
    /// The scheduler's loop.
    pub(super) fn run(&'static self, ctx: &editor_task_queue::TaskContext) {
        profiling::set_thread_name("Helio Runtimes");
        let _timer = FineTimerResolution::acquire();
        let mut plugin_generation = pulsar_plugins::generation();
        while !self.stop.load(Ordering::Acquire) {
            ctx.heartbeat();
            let generation = pulsar_plugins::generation();
            if generation != plugin_generation {
                plugin_generation = generation;
                self.destroy_disabled_owners();
            }
            let mut runtimes: Vec<Arc<Runtime>> = self
                .runtimes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            runtimes.sort_by_key(|runtime| {
                -runtime
                    .description
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .priority
            });
            let now = Instant::now();
            let mut next = now + IDLE_WAIT;
            let mut drew = false;
            for runtime in &runtimes {
                match self.step(runtime, now) {
                    Step::Drew(at) => {
                        drew = true;
                        next = next.min(at);
                    }
                    Step::DueAt(at) => next = next.min(at),
                    Step::Idle => {}
                }
            }
            ctx.set_idle(!drew && runtimes.iter().all(|r| r.state() != RuntimeState::Running));
            self.sleep_until(next);
        }
    }

    fn sleep_until(&self, until: Instant) {
        let (flag, condvar) = &self.wake;
        let mut woken = flag.lock().unwrap_or_else(|e| e.into_inner());
        while !*woken {
            let now = Instant::now();
            if now >= until {
                break;
            }
            woken = condvar
                .wait_timeout(woken, until - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *woken = false;
    }

    fn destroy_disabled_owners(&self) {
        let mut runtimes = self.runtimes.lock().unwrap_or_else(|e| e.into_inner());
        runtimes.retain(|runtime| {
            let origin = runtime
                .description
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .origin;
            let keep = pulsar_plugins::origin_enabled(origin);
            if !keep {
                runtime
                    .task
                    .context()
                    .warn("destroyed: the plugin that owns it is no longer enabled");
                runtime.set_state(RuntimeState::Stopped);
                runtime.task.finish();
            }
            keep
        });
    }

    /// One runtime's turn in a pass.
    fn step(&self, runtime: &Arc<Runtime>, now: Instant) -> Step {
        let held = runtime
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match held {
            Some(RuntimeState::Paused) => {
                runtime.set_state(RuntimeState::Paused);
                return Step::Idle;
            }
            Some(RuntimeState::Stopped) => {
                if runtime.state() != RuntimeState::Stopped {
                    if let Ok(mut renderer) = runtime.renderer.try_lock() {
                        renderer.release_gpu_resources();
                        runtime.set_state(RuntimeState::Stopped);
                        runtime.task.context().info("stopped: GPU resources freed");
                    }
                }
                return Step::Idle;
            }
            _ => {}
        }
        let Some(output) = runtime
            .output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Step::Idle;
        };
        if output.is_lost() {
            if runtime.state() != RuntimeState::Lost {
                runtime
                    .task
                    .context()
                    .error("the output's GPU device was lost");
                runtime.set_state(RuntimeState::Lost);
            }
            return Step::Idle;
        }
        if matches!(
            runtime.state(),
            RuntimeState::Lost | RuntimeState::Failed(_)
        ) {
            return Step::Idle;
        }
        let (width, height) = output.size();
        if width == 0 || height == 0 {
            return Step::Idle;
        }
        // A build in flight holds the renderer: nothing to do until it is done.
        let Ok(needs_build) = runtime
            .renderer
            .try_lock()
            .map(|mut renderer| renderer.needs_build())
        else {
            return Step::Idle;
        };
        if needs_build {
            // A first build, the one after a stop, a pipeline swap or changed
            // graph settings compiles pipelines: a pooled task, so the other
            // runtimes keep drawing (the one being rebuilt shows its last
            // frame meanwhile).
            self.build_in_background(runtime, &output);
            return Step::Idle;
        }
        if runtime.state() != RuntimeState::Running {
            runtime.set_state(RuntimeState::Running);
        }
        if !runtime.wants_frames() {
            runtime.set_quiet(true);
            return Step::Idle;
        }
        // The one frame policy (`pacing`): a visible viewport of a minimized
        // window sleeps, of a background window draws slowly, of a window in
        // use draws as fast as it has something new to show.
        let base = output.frame_interval();
        let windowed = runtime
            .description
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode
            == UpdateMode::WhenVisible;
        let interval = if windowed {
            match super::pacing::current().pace(base, runtime.presence(), runtime.frame_kind()) {
                super::Pace::Asleep => {
                    runtime.set_quiet(true);
                    return Step::Idle;
                }
                super::Pace::Every(interval) => interval,
            }
        } else {
            base
        };
        runtime.set_quiet(false);
        if let Err(at) = runtime.due(now, interval) {
            return Step::DueAt(at);
        }
        if !output.ready() {
            // The consumer still holds the last frame: its waker
            // (`set_ready_waker`) wakes the scheduler when it takes it; this
            // retry only bounds an output that has none.
            return Step::DueAt(now + interval / 4);
        }
        self.draw(runtime, &output, now);
        // Next due one frame on, at the pace the frame just drawn decided.
        let interval = if windowed {
            match super::pacing::current().pace(base, runtime.presence(), runtime.frame_kind()) {
                super::Pace::Asleep => IDLE_WAIT,
                super::Pace::Every(interval) => interval,
            }
        } else {
            base
        };
        Step::Drew(now + interval)
    }

    fn build_in_background(&self, runtime: &Arc<Runtime>, output: &Arc<dyn FrameTarget>) {
        if runtime.building.load(Ordering::Acquire) || self.building.swap(true, Ordering::AcqRel) {
            return;
        }
        runtime.building.store(true, Ordering::Release);
        let runtime = Arc::clone(runtime);
        let output = Arc::clone(output);
        let label = runtime
            .description
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .label
            .clone();
        // Only a first build is "starting": a rebuild keeps the runtime
        // running on its last frame.
        if runtime
            .renderer
            .try_lock()
            .is_ok_and(|renderer| !renderer.is_initialized())
        {
            runtime.set_state(RuntimeState::Starting);
        }
        editor_task_queue::global().submit(
            editor_task_queue::TaskDescription::new(
                format!("Build Helio renderer: {label}"),
                "render",
                editor_task_queue::TaskDuration::Long,
            ),
            move |task| {
                task.set_phase("Creating the scene renderer and render graph");
                let (device, queue) = (output.device(), output.queue());
                let (width, height) = output.size();
                // A panic in a pass constructor is the build's failure, not the
                // scheduler's: the flags below are released either way.
                let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _span = runtime.task.context().span().entered();
                    runtime
                        .renderer
                        .lock()
                        .map(|mut renderer| {
                            renderer.prepare_for_render(
                                &device,
                                &queue,
                                width,
                                height,
                                output.format(),
                            )
                        })
                        .unwrap_or(false)
                }));
                runtime.building.store(false, Ordering::Release);
                global().building.store(false, Ordering::Release);
                let result = match built {
                    Ok(true) => {
                        runtime.set_state(RuntimeState::Running);
                        Ok(())
                    }
                    // Its output's device was lost meanwhile: the next pass
                    // says so and a new output builds it again.
                    Ok(false) if output.is_lost() => Ok(()),
                    Ok(false) => Err("the renderer could not be built".to_string()),
                    Err(_) => Err("building the renderer panicked".to_string()),
                };
                if let Err(reason) = &result {
                    runtime.task.context().error(reason);
                    runtime.set_state(RuntimeState::Failed(reason.clone()));
                }
                runtime.publish_details();
                global().wake();
                result
            },
        );
    }

    fn draw(&self, runtime: &Arc<Runtime>, output: &Arc<dyn FrameTarget>, now: Instant) {
        *runtime.last_frame.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
        // The renderer's own logs while it draws belong to this runtime's task.
        let _span = runtime.task.context().span().entered();
        let Ok(mut renderer) = runtime.renderer.try_lock() else {
            // The owner holds it (an edit, a capture): next pass.
            return;
        };
        let started = Instant::now();
        tracing::trace!(target: "render_runtimes", runtime = runtime.id, "draw: renderer locked");
        if runtime.reset_history.swap(false, Ordering::AcqRel) {
            renderer.reset_taa();
        }
        // The pace this frame is drawn at, for passes that stream work in
        // (none when unpaced).
        renderer.set_frame_budget(Some(output.frame_interval()).filter(|i| !i.is_zero()));
        if let Some(callback) = runtime
            .before_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            callback();
        }
        tracing::trace!(target: "render_runtimes", runtime = runtime.id, "draw: input latched");
        let (device, queue, format) = (output.device(), output.queue(), output.format());
        let (width, height) = output.size();
        // A pipeline swap or project-setting rebuild happens here, at a frame
        // boundary, before the frame takes the output's submit permission.
        if !renderer.prepare_for_render(&device, &queue, width, height, format) {
            return;
        }
        tracing::trace!(target: "render_runtimes", runtime = runtime.id, "draw: prepared");
        let published = output.draw(&mut |view, width, height| {
            tracing::trace!(target: "render_runtimes", "draw: output acquired");
            let submitted =
                renderer.render_frame_to_surface(&device, &queue, view, width, height, format);
            tracing::trace!(target: "render_runtimes", "draw: rendered");
            submitted
        });
        tracing::trace!(target: "render_runtimes", runtime = runtime.id, published, "draw: done");
        // What the frame was for decides the next one's pace.
        runtime
            .kind
            .store(renderer.frame_kind().to_bits(), Ordering::Release);
        drop(renderer);
        let took = started.elapsed();
        runtime
            .frame_us
            .store(took.as_micros() as u64, Ordering::Relaxed);
        if published {
            let frames = runtime.frames.fetch_add(1, Ordering::AcqRel) + 1;
            runtime.task.context().tick(took);
            if let UpdateMode::OnDemand(_) = runtime
                .description
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .mode
            {
                let _ = runtime
                    .demanded
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                        left.checked_sub(1)
                    });
            }
            // The details are a snapshot for the Renderers view: a few times
            // a second is plenty.
            if frames % 30 == 1 {
                runtime.publish_details();
            }
        }
    }
}

enum Step {
    /// Drew a frame; the next is due at the instant given.
    Drew(Instant),
    DueAt(Instant),
    Idle,
}
