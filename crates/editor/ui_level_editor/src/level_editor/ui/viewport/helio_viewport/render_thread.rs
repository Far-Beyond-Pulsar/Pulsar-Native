use super::*;
use super::frame_pacer::FramePacer;

impl HelioViewport {

    /// Start a dedicated background thread that continuously renders the Helio
    /// scene into the given `WgpuSurfaceHandle` and presents each frame.
    pub(super) fn start_render_thread(&mut self, surface: WgpuSurfaceHandle, refresh_hz: Option<f64>) {
        if self.render_thread_handle.is_some() {
            return;
        }

        let engine = self.gpu_engine.clone();
        let stop = self.render_thread_stop.clone();
        let tab_activated = self.tab_activated.clone();
        let frames_published = self.frames_published.clone();

        let handle = std::thread::Builder::new()
            .name("Helio Render".into())
            .spawn(move || {
                profiling::set_thread_name("Helio Render");

                let panic_result = catch_unwind(AssertUnwindSafe(|| {
                    // Start at the display's refresh rate and drop from there only
                    // if the renderer can't keep up. The old `sleep(16ms)` ran *in
                    // addition to* the render, so the real period was 16ms + render
                    // time: a scene taking 6ms to draw capped out around 45 FPS
                    // with the GPU idle most of the frame, and on a 144 Hz display
                    // it was leaving more than half the refresh rate on the table.
                    let mut pacer = FramePacer::new(refresh_hz);
                    tracing::info!(
                        "[VIEWPORT PACER] starting at {:.0} Hz{}",
                        pacer.target_hz,
                        if refresh_hz.is_some() {
                            " (display refresh)"
                        } else {
                            " (no refresh rate reported, using fallback)"
                        }
                    );

                    loop {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }

                        {
                            profiling::profile_scope!("Helio: frame pacer wait");
                            pacer.wait_for_next_frame();
                        }

                        // Backpressure: don't produce a frame the compositor hasn't
                        // asked for yet.
                        //
                        // The triple buffer holds exactly one `ready` frame. Publishing
                        // a second before the first is composited recycles the
                        // unconsumed buffer as the next render target — those pixels
                        // are thrown away, so rendering faster than the compositor
                        // consumes can never raise the displayed frame rate. What it
                        // does do is keep issuing GPU submissions and per-frame
                        // allocations that nothing retires at the same rate, which is
                        // what makes an uncapped render thread climb in memory until
                        // the process stalls.
                        //
                        // Waiting here instead makes the producer self-pace to the
                        // consumer's actual rate. The wait is bounded: the fast-blit
                        // presentation path never advances the composited generation,
                        // so an unbounded wait there would stall the viewport for good.
                        let should_render = {
                            profiling::profile_scope!("Helio: wait for compositor to consume frame");
                            wait_for_frame_consumed(&surface, &stop, CONSUMER_WAIT_TIMEOUT)
                        };
                        if !should_render {
                            continue;
                        }

                        // Permission to submit on the shared device. Held across
                        // render + present so a window resize (which reconfigures the
                        // swapchain and requires an idle queue) cannot race our submit.
                        // This is a read guard: other surfaces' render threads hold
                        // theirs concurrently, so frame pacing stays independent.
                        let _frame = gpui::render_stats::scope("helio: FRAME TOTAL");
                        gpui::render_stats::count("helio frames rendered");
                        // Blocks for as long as a window resize holds the
                        // exclusive side of this lock (`Surface::configure`).
                        let submit_guard = {
                            profiling::profile_scope!("Helio: wait gpu_submit_lock (read)");
                            surface.submit_guard()
                        };

                        let back = {
                            profiling::profile_scope!("Helio: acquire back buffer");
                            surface.back_view_with_size()
                        };
                        let Some((view, (width, height))) = back else {
                            gpui::render_stats::count("helio: no back buffer (skipped)");
                            continue;
                        };

                        let device = surface.device();
                        let queue = surface.queue();
                        let format = surface.format();

                        let submission_index = {
                            let lock_start = Instant::now();
                            let locked = {
                                profiling::profile_scope!("Helio: wait for engine lock");
                                engine.lock()
                            };
                            gpui::render_stats::record(
                                "helio: wait for engine lock",
                                lock_start.elapsed(),
                            );

                            let mut engine = match locked {
                                Ok(engine) => engine,
                                Err(poisoned) => {
                                    tracing::error!(
                                        "[HELIO-VIEWPORT] renderer mutex was poisoned; recovering"
                                    );
                                    poisoned.into_inner()
                                }
                            };
                            if tab_activated.swap(false, Ordering::AcqRel) {
                                engine.reset_taa();
                            }
                            let _t = gpui::render_stats::scope("helio: render_frame_to_surface");
                            engine.render_frame_to_surface(
                                device, queue, &view, width, height, format,
                            )
                        };

                        drop(view);

                        if let Some(idx) = submission_index {
                            // Silent present: publish the frame for the compositor but do
                            // NOT request a window redraw from this thread. Driving
                            // repaints from here would fire a winit `RedrawRequested` per
                            // frame, and each one that misses the fast-blit path forces a
                            // full `window.refresh()` of the entire editor UI.
                            //
                            // Instead we bump `frames_published`; the UI-thread frame pump
                            // sees the change and repaints just this view. Release ordering
                            // pairs with the pump's acquire load so the swapped buffer is
                            // visible before the counter is.
                            {
                                profiling::profile_scope!("Helio: present_synced_silent");
                                surface.present_synced_silent(idx);
                            }
                            frames_published.fetch_add(1, Ordering::Release);
                        }

                        drop(submit_guard);
                    }
                }));

                if let Err(payload) = panic_result {
                    let message = if let Some(message) = payload.downcast_ref::<&str>() {
                        (*message).to_string()
                    } else if let Some(message) = payload.downcast_ref::<String>() {
                        message.clone()
                    } else {
                        "non-string panic payload".to_string()
                    };
                    let backtrace = std::backtrace::Backtrace::force_capture();
                    let report =
                        format!("Helio render thread panicked: {message}\nBacktrace:\n{backtrace}");

                    // This is intentionally unconditional: a logging filter must
                    // not hide the fact that the renderer thread has terminated.
                    eprintln!("[HELIO RENDER THREAD PANIC] {report}");
                    tracing::error!("[HELIO RENDER THREAD PANIC] {report}");
                }
            });

        match handle {
            Ok(h) => self.render_thread_handle = Some(h),
            Err(e) => tracing::error!("Failed to spawn Helio render thread: {:?}", e),
        }
    }

    /// Start the once-per-view `on_next_frame` pump that repaints this view when
    /// the render thread has published a frame.
    ///
    /// Notifying only on a counter change is what keeps the rest of the editor
    /// cached: `Window::mark_view_dirty` walks the ancestor path, so every
    /// notify here also invalidates the viewport panel, the tab panel, the
    /// workspace and `LevelEditorPanel`. At Helio's ~60 FPS that is one such
    /// cascade per rendered frame instead of one per event-loop iteration.
    ///
    /// `awaiting_render` throttles the pump to at most one outstanding repaint.
    /// Without it, a viewport sitting in an inactive tab — where `render()` is
    /// never reached but the render thread keeps publishing — would notify on
    /// every single frame. The flag clears in `render()`, so the pump resumes as
    /// soon as the tab is shown again.
    pub(super) fn start_frame_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        crate::level_editor::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, window, cx| {
                let published = this.frames_published.load(Ordering::Acquire);
                if published == this.last_published_frame {
                    return;
                }
                this.last_published_frame = published;

                // A published texture changes no element state, so the frame
                // only needs compositing — `refresh_buffers` marks the window
                // dirty without marking any view dirty, and every cached view
                // replays instead of rebuilding. Scene content (gizmo drags,
                // camera moves) reaches the texture through that path alone;
                // it never needs a view rebuild.
                //
                // But a view that never prepaints never observes new bounds,
                // and `WgpuSurface::prepaint` is what resizes the surface to
                // match its element. So a real notify is issued when the
                // window viewport size changed since the last full render
                // (window resize / maximize), or once per
                // `FULL_RENDER_INTERVAL` published frames as the fallback for
                // geometry changes with no size signal — panel splits and
                // undocks. That bounds their pickup latency without letting
                // the ancestor-chain rebuild dominate idle frames.
                this.frames_since_full_render += 1;
                let viewport_resized =
                    Some(window.viewport_size()) != this.viewport_size_at_last_full_render;
                if this.frames_since_full_render >= FULL_RENDER_INTERVAL
                    || this.awaiting_render
                    || viewport_resized
                {
                    this.viewport_size_at_last_full_render = Some(window.viewport_size());
                    this.awaiting_render = true;
                    cx.notify();
                } else {
                    window.refresh_buffers();
                }
            },
        );
    }

    fn record_frame_diagnostics(
        &mut self,
        total_ms: f64,
        acquire_ms: f64,
        render_ms: f64,
        swap_ms: f64,
        engine_lock_missed: bool,
    ) {
        if engine_lock_missed {
            self.engine_lock_misses_since_report += 1;
        }
        if total_ms > 33.3 {
            self.slow_frames_since_report += 1;
            self.max_frame_ms_since_report = self.max_frame_ms_since_report.max(total_ms);
        }

        if (self.slow_frames_since_report > 0 || self.engine_lock_misses_since_report > 0)
            && self.last_spike_report.elapsed().as_secs_f32() >= 1.0
        {
            tracing::warn!(
                "[VIEWPORT SPIKES] slow={} lock_misses={} max={:.1}ms latest={:.1}ms (acquire {:.1}, render {:.1}, swap {:.1})",
                self.slow_frames_since_report,
                self.engine_lock_misses_since_report,
                self.max_frame_ms_since_report,
                total_ms,
                acquire_ms,
                render_ms,
                swap_ms
            );
            self.last_spike_report = Instant::now();
            self.slow_frames_since_report = 0;
            self.engine_lock_misses_since_report = 0;
            self.max_frame_ms_since_report = 0.0;
        }
    }
}


/// How long to wait for the compositor to consume the previously published
/// frame before rendering anyway.
///
/// Long enough that a normal 60 Hz consumer is never rushed, short enough that
/// the viewport keeps updating if the composited generation stops advancing
/// (the fast-blit path, a stalled UI thread, a hidden tab).
const CONSUMER_WAIT_TIMEOUT: Duration = Duration::from_millis(100);

/// Block until the compositor has promoted the last published frame, `stop` is
/// signalled, or `timeout` elapses.
///
/// Returns `true` if the caller should go on to render this iteration, `false`
/// if it should re-check the stop flag and start over.
fn wait_for_frame_consumed(
    surface: &WgpuSurfaceHandle,
    stop: &Arc<AtomicBool>,
    timeout: Duration,
) -> bool {
    if !surface.has_unconsumed_frame() {
        return true;
    }

    let deadline = Instant::now() + timeout;
    while surface.has_unconsumed_frame() {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        if Instant::now() >= deadline {
            // Consumer isn't advancing. Render anyway rather than freeze the
            // viewport; the frame may be discarded, but at display rates the
            // waste is bounded by this timeout.
            gpui::render_stats::count("helio: consumer wait timed out");
            return true;
        }
        // Park briefly rather than spin: this thread has nothing to do until
        // the compositor runs, and busy-waiting would burn a core to no end.
        std::thread::sleep(Duration::from_micros(500));
    }

    true
}

/// Published frames between full re-renders of the viewport view.
///
/// Between these the pump uses `Window::refresh_buffers`, which composites the
/// new texture without invalidating any view. The periodic full render exists
/// solely so `WgpuSurface::prepaint` runs often enough to keep the surface
/// sized to its element — a purely buffer-refreshed viewport never observes
/// new bounds. Window-level resizes bypass the wait entirely (the pump
/// compares `Window::viewport_size` against the last full render), so this
/// interval only governs geometry changes with no size signal, such as panel
/// splits.
///
/// At ~60-85 FPS this is roughly one ancestor-chain rebuild every 2 seconds
/// instead of ~six per second; those rebuilds were a measurable share of the
/// editor's frame cost during notify storms.
const FULL_RENDER_INTERVAL: u32 = 150;
