//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! Follows the reference example `wgpu_surface_basic.rs`:
//!   1. Create `WgpuSurfaceHandle` lazily on first render.
//!   2. Each frame: `back_view_with_size()` → render → `swap_buffers()`.
//!   3. Return `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use plugin_editor_api::{AssetKind, AssetPayload};
use rust_i18n::t;
use ui::{ActiveTheme as _, ContextModal, notification::Notification};

use crate::level_editor::commands::{SceneCommand, execute_command};
use crate::level_editor::scene_database::{MeshType, ObjectType, SceneObjectData, Transform};
use crate::level_editor::state::LevelEditorState;
use helio_component::asset_component::component_class_for_asset;
use pulsar_reflection::REGISTRY;

/// A GPUI component that drives the Helio renderer into a `WgpuSurfaceHandle`.
///
/// Rendering runs on a dedicated background thread so the UI thread is never
/// blocked by GPU work. The background thread owns a clone of the `GpuRenderer`
/// and the `WgpuSurfaceHandle`; it loops at ~60 FPS calling
/// `render_frame_to_surface()` + `present_synced_silent()`.
///
/// Presentation is deliberately split across the two threads:
/// - the render thread only publishes finished frames into the triple buffer
///   and bumps `frames_published`;
/// - the UI thread runs a frame pump that watches that counter and repaints
///   only when a new frame actually exists, so the compositor promotes
///   `ready` -> `display` exactly once per rendered frame.
///
/// The pump is deliberately *not* `Window::request_animation_frame()` in
/// `render()`. That notifies this view every single frame, and
/// `Window::mark_view_dirty` propagates dirt to every ancestor — the viewport
/// panel, the tab panel, the workspace and `LevelEditorPanel` itself — so the
/// entire level editor chrome was being rebuilt at event-loop rate regardless
/// of whether Helio had produced anything new.
pub struct HelioViewport {
    pub gpu_engine: Arc<Mutex<GpuRenderer>>,
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    surface: Option<WgpuSurfaceHandle>,
    focus_handle: FocusHandle,
    debug_replace_with_yellow: bool,
    tab_activated: Arc<AtomicBool>,
    render_thread_stop: Arc<AtomicBool>,
    render_thread_handle: Option<std::thread::JoinHandle<()>>,
    /// Incremented by the render thread each time it publishes a frame into the
    /// triple buffer. Read by the frame pump to decide whether a repaint is
    /// warranted.
    frames_published: Arc<AtomicU64>,
    /// Value of `frames_published` at the last repaint this view requested.
    last_published_frame: u64,
    /// A repaint has been requested but `render()` has not run yet. Keeps the
    /// pump from stacking up notifies for a viewport that isn't being rendered.
    awaiting_render: bool,
    /// Published frames since this view last actually rendered. Drives the
    /// periodic real notify that keeps surface bounds tracking its element.
    frames_since_full_render: u32,
    /// Window viewport size at the last full render. A change means the
    /// element's bounds are likely stale, so the next pump tick promotes a
    /// real notify instead of waiting out the frame interval.
    viewport_size_at_last_full_render: Option<Size<Pixels>>,
    /// Whether the `on_next_frame` pump has been started (once per view).
    pump_started: bool,
    last_spike_report: Instant,
    slow_frames_since_report: u32,
    engine_lock_misses_since_report: u32,
    max_frame_ms_since_report: f64,
}

impl HelioViewport {
    pub fn new<V: 'static>(
        gpu_engine: Arc<Mutex<GpuRenderer>>,
        shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
        debug_replace_with_yellow: bool,
        cx: &mut Context<V>,
    ) -> Self {
        Self {
            gpu_engine,
            shared_state,
            surface: None,
            focus_handle: cx.focus_handle(),
            debug_replace_with_yellow,
            tab_activated: Arc::new(AtomicBool::new(true)),
            render_thread_stop: Arc::new(AtomicBool::new(false)),
            render_thread_handle: None,
            frames_published: Arc::new(AtomicU64::new(0)),
            last_published_frame: 0,
            awaiting_render: false,
            frames_since_full_render: 0,
            viewport_size_at_last_full_render: None,
            pump_started: false,
            last_spike_report: Instant::now(),
            slow_frames_since_report: 0,
            engine_lock_misses_since_report: 0,
            max_frame_ms_since_report: 0.0,
        }
    }

    /// Mark the viewport as having been activated (e.g. tab switch).
    /// The next render will reset TAA history to prevent ghosting.
    pub fn mark_tab_activated(&mut self) {
        self.tab_activated.store(true, Ordering::Release);
    }

    /// Handle an asset being dropped on the viewport
    fn handle_asset_drop(
        &mut self,
        payload: &AssetPayload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = PathBuf::from(&payload.engine_path);
        let name = payload.name.clone();
        let kind = payload.kind.clone();

        tracing::info!("Asset dropped on viewport: {} ({:?})", name, kind);

        window.push_notification(
            Notification::info(t!("Notification.Title.AddingToScene").to_string())
                .message(t!("Notification.Message.Placing", name => &name).to_string()),
            cx,
        );

        let result = Self::import_asset(path, kind, self.shared_state.clone());
        match result {
            Ok(()) => {
                window.push_notification(
                    Notification::success(t!("Notification.Title.AddedToScene").to_string())
                        .message(t!("Notification.Message.Placed", name => &name).to_string()),
                    cx,
                );
            }
            Err(e) => {
                tracing::error!("Failed to place {}: {}", name, e);
                window.push_notification(
                    Notification::error(t!("Notification.Title.PlacementFailed").to_string())
                        .message(
                            t!(
                                "Notification.Message.FailedToPlace",
                                name => &name,
                                error => e.to_string()
                            )
                            .to_string(),
                        ),
                    cx,
                );
            }
        }
    }

    /// Insert the dropped asset into the scene via the central SceneDatabase API.
    ///
    /// All assets — meshes, FBX files, blueprints — are inserted as SceneObjectData
    /// entries with the appropriate component instances.  The renderer's sync_scene()
    /// loop handles the actual GPU work every frame; nothing writes directly to Helio
    /// from this path.
    fn import_asset(
        path: PathBuf,
        kind: AssetKind,
        shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()).into());
        }

        match kind {
            AssetKind::Mesh | AssetKind::Scene => {
                let asset_path = path.to_string_lossy().replace('\\', "/");
                let imported_name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Imported Asset")
                    .to_string();

                let mut state = shared_state.write();
                let mesh_object = SceneObjectData {
                    id: String::new(),
                    name: imported_name,
                    object_type: ObjectType::Mesh(MeshType::Custom),
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    props: std::collections::HashMap::new(),
                    scene_path: path.display().to_string(),
                    component_instances: None,
                };

                let add_result = execute_command(
                    &mut state,
                    SceneCommand::AddObject {
                        data: mesh_object,
                        parent_id: None,
                    },
                );

                if let Some(id) = add_result.affected_ids.first() {
                    if let Some((class_name, data_field)) = component_class_for_asset(&kind) {
                        if REGISTRY.has_class(class_name) {
                            state.scene.database.add_component(
                                id,
                                class_name.to_string(),
                                serde_json::json!({ data_field: asset_path }),
                            );
                            let _ = execute_command(
                                &mut state,
                                SceneCommand::SelectObject {
                                    id: Some(id.clone()),
                                },
                            );
                        }
                    }
                }
            }
            AssetKind::Blueprint => {
                if !path.is_dir() {
                    return Err(
                        format!("Blueprint path is not a directory: {}", path.display()).into(),
                    );
                }
                if !path.join("graph_save.json").exists() {
                    return Err(format!(
                        "Not a valid blueprint class (missing graph_save.json): {}",
                        path.display()
                    )
                    .into());
                }

                let class_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let script_path = path.to_string_lossy().replace('\\', "/");

                let mut state = shared_state.write();
                let blueprint_object = SceneObjectData {
                    id: String::new(),
                    name: class_name,
                    object_type: ObjectType::Blueprint,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    props: std::collections::HashMap::new(),
                    scene_path: path.display().to_string(),
                    component_instances: None,
                };

                let add_result = execute_command(
                    &mut state,
                    SceneCommand::AddObject {
                        data: blueprint_object,
                        parent_id: None,
                    },
                );

                if let Some(id) = add_result.affected_ids.first() {
                    if let Some((class_name, data_field)) = component_class_for_asset(&kind) {
                        if REGISTRY.has_class(class_name) {
                            state.scene.database.add_component(
                                id,
                                class_name.to_string(),
                                serde_json::json!({ data_field: script_path }),
                            );
                            let _ = execute_command(
                                &mut state,
                                SceneCommand::SelectObject {
                                    id: Some(id.clone()),
                                },
                            );
                        }
                    }
                }
            }
            _ => {
                return Err(format!("Unsupported asset type: {:?}", kind).into());
            }
        }

        Ok(())
    }

    /// Start a dedicated background thread that continuously renders the Helio
    /// scene into the given `WgpuSurfaceHandle` and presents each frame.
    fn start_render_thread(&mut self, surface: WgpuSurfaceHandle, refresh_hz: Option<f64>) {
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

                        pacer.wait_for_next_frame();

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
                        if !wait_for_frame_consumed(&surface, &stop, CONSUMER_WAIT_TIMEOUT) {
                            continue;
                        }

                        // Permission to submit on the shared device. Held across
                        // render + present so a window resize (which reconfigures the
                        // swapchain and requires an idle queue) cannot race our submit.
                        // This is a read guard: other surfaces' render threads hold
                        // theirs concurrently, so frame pacing stays independent.
                        let _frame = gpui::render_stats::scope("helio: FRAME TOTAL");
                        gpui::render_stats::count("helio frames rendered");
                        let submit_guard = surface.submit_guard();

                        let Some((view, (width, height))) = surface.back_view_with_size() else {
                            gpui::render_stats::count("helio: no back buffer (skipped)");
                            continue;
                        };

                        let device = surface.device();
                        let queue = surface.queue();
                        let format = surface.format();

                        let submission_index = {
                            let lock_start = Instant::now();
                            let locked = engine.lock();
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
                            surface.present_synced_silent(idx);
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
    fn start_frame_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let viewport_resized = Some(window.viewport_size())
                    != this.viewport_size_at_last_full_render;
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

/// Fallback target when the platform doesn't report a refresh rate.
const FALLBACK_REFRESH_HZ: f64 = 60.0;

/// Never pace slower than this, however badly frames are missing. Below it the
/// viewport stops feeling interactive and dropping further doesn't help.
const MIN_TARGET_HZ: f64 = 20.0;

/// Adaptive frame pacer for the Helio render thread.
///
/// Starts at the display's refresh rate — presenting faster cannot show more
/// frames, since the compositor promotes at most one Helio frame per refresh
/// and surplus frames are overwritten in the triple buffer's `ready` slot — and
/// backs off when the renderer can't sustain it, recovering when it can.
///
/// Backing off is not cosmetic. A thread that keeps aiming at a rate it cannot
/// hit spends every frame late, never sleeps, and turns into the unbounded
/// producer that `wait_for_frame_consumed` exists to prevent. Dropping the
/// target restores the idle gap between frames.
struct FramePacer {
    /// The rate we'd like to hit — the display refresh, unless overridden.
    ceiling_hz: f64,
    /// The rate we're currently aiming at, `<= ceiling_hz`.
    target_hz: f64,
    /// When the next frame is due.
    next_deadline: Instant,
    /// Consecutive frames that overran their budget.
    late_streak: u32,
    /// Consecutive frames that met their deadline at the current target.
    on_time_streak: u32,
}

impl FramePacer {
    /// Frames in a row that must overrun before dropping the target.
    const LATE_STREAK_TO_DROP: u32 = 8;
    /// Frames in a row that must meet their deadline before testing the ceiling
    /// again. Recovery is a single jump; if the ceiling is unstable, the
    /// late-frame logic backs it off again instead of slowly curving upward.
    const ON_TIME_STREAK_TO_RAISE: u32 = 30;
    /// Multiplier applied on each drop.
    const DROP_FACTOR: f64 = 0.8;

    fn new(refresh_hz: Option<f64>) -> Self {
        // `PULSAR_VIEWPORT_FPS` overrides the ceiling; `0` disables pacing and
        // lets `wait_for_frame_consumed` be the only thing governing the rate.
        let override_hz = std::env::var("PULSAR_VIEWPORT_FPS")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v >= 0.0);

        Self::with_ceiling(match override_hz {
            Some(hz) => hz,
            None => refresh_hz.unwrap_or(FALLBACK_REFRESH_HZ),
        })
    }

    /// Build a pacer aiming at `ceiling_hz`, ignoring the environment. Split out
    /// from [`new`](Self::new) so the adaptation logic is testable without an
    /// ambient `PULSAR_VIEWPORT_FPS` changing the outcome.
    fn with_ceiling(ceiling_hz: f64) -> Self {
        Self {
            ceiling_hz,
            target_hz: ceiling_hz,
            next_deadline: Instant::now(),
            late_streak: 0,
            on_time_streak: 0,
        }
    }

    fn budget(&self) -> Option<Duration> {
        if self.target_hz <= 0.0 {
            None
        } else {
            Some(Duration::from_secs_f64(1.0 / self.target_hz))
        }
    }

    /// Sleep until this frame is due. Returns immediately when uncapped or when
    /// the deadline has already passed.
    fn wait_for_next_frame(&mut self) {
        let Some(budget) = self.budget() else {
            return;
        };

        let now = Instant::now();
        if self.next_deadline > now {
            // Anchoring to the previous deadline (rather than sleeping a fixed
            // amount after the render) keeps the average rate on target and
            // absorbs overshoot from Windows' coarse timer granularity on the
            // following frame.
            std::thread::sleep(self.next_deadline - now);
            self.on_time_streak = self.on_time_streak.saturating_add(1);
            self.late_streak = 0;
        } else {
            // Missed the deadline: the previous frame ran long.
            self.late_streak = self.late_streak.saturating_add(1);
            self.on_time_streak = 0;
        }

        self.next_deadline += budget;
        let now = Instant::now();
        if self.next_deadline < now {
            // More than a frame behind — resync instead of trying to catch up
            // with a burst of back-to-back frames.
            self.next_deadline = now + budget;
        }

        self.adapt();
    }

    fn adapt(&mut self) {
        if self.target_hz <= 0.0 {
            return;
        }

        if self.late_streak >= Self::LATE_STREAK_TO_DROP {
            let dropped = (self.target_hz * Self::DROP_FACTOR).max(MIN_TARGET_HZ);
            if dropped < self.target_hz {
                tracing::debug!(
                    "[VIEWPORT PACER] target {:.0} -> {:.0} Hz (missed {} frames in a row)",
                    self.target_hz,
                    dropped,
                    self.late_streak
                );
                self.target_hz = dropped;
            }
            self.late_streak = 0;
        } else if self.on_time_streak >= Self::ON_TIME_STREAK_TO_RAISE
            && self.target_hz < self.ceiling_hz
        {
            tracing::debug!(
                "[VIEWPORT PACER] target {:.0} -> {:.0} Hz (stability window passed)",
                self.target_hz,
                self.ceiling_hz
            );
            self.target_hz = self.ceiling_hz;
            self.on_time_streak = 0;
        }
    }
}

#[cfg(test)]
mod pacer_tests {
    use super::{FALLBACK_REFRESH_HZ, FramePacer, MIN_TARGET_HZ};

    /// Drive `adapt` as if `n` frames in a row missed their deadline.
    fn run_late_frames(pacer: &mut FramePacer, n: u32) {
        for _ in 0..n {
            pacer.late_streak += 1;
            pacer.on_time_streak = 0;
            pacer.adapt();
        }
    }

    /// Drive `adapt` as if `n` frames in a row met their deadline.
    fn run_on_time_frames(pacer: &mut FramePacer, n: u32) {
        for _ in 0..n {
            pacer.on_time_streak += 1;
            pacer.late_streak = 0;
            pacer.adapt();
        }
    }

    #[test]
    fn starts_at_the_display_refresh_rate() {
        assert_eq!(FramePacer::with_ceiling(144.0).target_hz, 144.0);
        assert_eq!(FramePacer::with_ceiling(240.0).target_hz, 240.0);
    }

    #[test]
    fn falls_back_when_the_platform_reports_no_refresh_rate() {
        // `new(None)` with no override must land on the fallback, not on zero.
        let pacer = FramePacer::with_ceiling(FALLBACK_REFRESH_HZ);
        assert_eq!(pacer.target_hz, 60.0);
    }

    #[test]
    fn holds_the_target_while_frames_are_on_time() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        run_on_time_frames(&mut pacer, 1000);
        assert_eq!(pacer.target_hz, 144.0, "should never exceed the ceiling");
    }

    #[test]
    fn a_short_burst_of_late_frames_does_not_drop_the_target() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP - 1);
        assert_eq!(pacer.target_hz, 144.0);
    }

    #[test]
    fn drops_the_target_after_sustained_late_frames() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP);
        assert!(
            pacer.target_hz < 144.0,
            "expected a drop, got {}",
            pacer.target_hz
        );
    }

    #[test]
    fn never_drops_below_the_floor() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        // Far more late frames than could ever be needed to bottom out.
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP * 500);
        assert!(
            pacer.target_hz >= MIN_TARGET_HZ,
            "target {} fell below the floor {}",
            pacer.target_hz,
            MIN_TARGET_HZ
        );
    }

    #[test]
    fn recovers_toward_the_ceiling_but_never_past_it() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP * 6);
        let dropped = pacer.target_hz;
        assert!(dropped < 144.0);

        run_on_time_frames(&mut pacer, FramePacer::ON_TIME_STREAK_TO_RAISE * 100);
        assert!(
            pacer.target_hz > dropped,
            "expected recovery from {}, got {}",
            dropped,
            pacer.target_hz
        );
        assert_eq!(
            pacer.target_hz, 144.0,
            "sustained headroom should return all the way to the display refresh"
        );
    }

    #[test]
    fn recovery_waits_then_jumps_to_the_ceiling() {
        assert!(FramePacer::ON_TIME_STREAK_TO_RAISE > FramePacer::LATE_STREAK_TO_DROP);

        let mut pacer = FramePacer::with_ceiling(144.0);
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP);
        let dropped = pacer.target_hz;

        run_on_time_frames(&mut pacer, FramePacer::ON_TIME_STREAK_TO_RAISE - 1);
        assert_eq!(pacer.target_hz, dropped);

        run_on_time_frames(&mut pacer, 1);
        assert_eq!(pacer.target_hz, 144.0);
    }

    #[test]
    fn an_uncapped_pacer_stays_uncapped() {
        // `PULSAR_VIEWPORT_FPS=0` hands pacing entirely to the consumer-side
        // backpressure; adapt must not resurrect a target from zero.
        let mut pacer = FramePacer::with_ceiling(0.0);
        assert!(pacer.budget().is_none());
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP * 4);
        assert!(pacer.budget().is_none());
        assert_eq!(pacer.target_hz, 0.0);
    }
}

impl Drop for HelioViewport {
    fn drop(&mut self) {
        self.render_thread_stop.store(true, Ordering::Release);
    }
}

/// Renders the current Helio scene into an offscreen texture, reads it back
/// from the GPU, and writes it to `out_path` as a PNG. Used to capture
/// project thumbnails on scene save.
fn capture_viewport_thumbnail(
    engine: &mut GpuRenderer,
    surface: &WgpuSurfaceHandle,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    out_path: &std::path::Path,
) {
    let device = surface.device();
    let queue = surface.queue();

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thumbnail-capture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let _ = engine.render_frame_to_surface(device, queue, &view, width, height, format);

    let bytes_per_row = align_up(width * 4, 256);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("thumbnail-staging"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("thumbnail-readback"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    match rx.recv() {
        Ok(Ok(())) => {}
        _ => {
            tracing::warn!("[THUMBNAIL] Failed to map readback buffer");
            return;
        }
    }

    let data = match slice.get_mapped_range() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("[THUMBNAIL] Failed to get mapped range: {:?}", e);
            return;
        }
    };
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        let end = start + (width * 4) as usize;
        pixels.extend_from_slice(&data[start..end]);
    }
    drop(data);
    staging.unmap();

    // The captured texture stores correctly sRGB-encoded bytes, but the live editor
    // viewport is composited via a shader that samples this `_Srgb` texture (auto
    // decoding sRGB -> linear) and writes the result directly into a non-sRGB
    // swapchain target (no re-encode). That makes the on-screen viewport appear
    // darker than the raw captured bytes. Apply the same sRGB -> linear decode here
    // so the saved thumbnail matches what the user actually sees in the editor.
    let srgb_to_linear_lut = srgb_to_linear_lut();
    for px in pixels.chunks_exact_mut(4) {
        px[0] = srgb_to_linear_lut[px[0] as usize];
        px[1] = srgb_to_linear_lut[px[1] as usize];
        px[2] = srgb_to_linear_lut[px[2] as usize];
    }

    let Some(rgba) = image::RgbaImage::from_raw(width, height, pixels) else {
        tracing::warn!(
            "[THUMBNAIL] Pixel buffer size mismatch for {}x{}",
            width,
            height
        );
        return;
    };

    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match rgba.save(out_path) {
        Ok(()) => tracing::info!(
            "[THUMBNAIL] Saved viewport thumbnail to {}",
            out_path.display()
        ),
        Err(e) => tracing::warn!("[THUMBNAIL] Failed to save {}: {}", out_path.display(), e),
    }
}

fn align_up(n: u32, align: u32) -> u32 {
    (n + align - 1) & !(align - 1)
}

/// Builds an 8-bit sRGB-decode (EOTF) lookup table, mapping each sRGB-encoded
/// byte value to its linear-light equivalent (also expressed as a byte 0-255).
fn srgb_to_linear_lut() -> [u8; 256] {
    let mut lut = [0u8; 256];
    for (i, entry) in lut.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        let linear = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
        *entry = (linear * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

impl Focusable for HelioViewport {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for HelioViewport {}

impl Render for HelioViewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        profiling::profile_scope!("helio_viewport_frame");
        if self.debug_replace_with_yellow {
            return div()
                .relative()
                .size_full()
                .track_focus(&self.focus_handle)
                .id("helio_viewport_debug_yellow")
                .bg(rgb(0xffff00))
                .into_any_element();
        }

        let format = wgpu::TextureFormat::Rgba8UnormSrgb;

        // Lazy surface creation (once) + start the background render thread.
        if self.surface.is_none() {
            // Read the refresh rate here, on the UI thread, while we still have
            // a `Window`: it's what the render thread starts its pacing at.
            let refresh_hz = window
                .display(cx)
                .and_then(|display| display.refresh_rate_millihertz())
                .map(|mhz| mhz as f64 / 1000.0)
                .filter(|hz| *hz > 0.0);

            match window.create_wgpu_surface(1600, 900, format) {
                Some(s) => {
                    tracing::info!("[HELIO-VIEWPORT] WgpuSurface created (format={:?})", format);
                    self.start_render_thread(s.clone(), refresh_hz);
                    self.surface = Some(s);
                }
                None => {
                    tracing::warn!("[HELIO-VIEWPORT] create_wgpu_surface returned None");
                }
            }
        }

        // Drain pending mesh-load errors (non-blocking).
        if let Ok(engine) = self.gpu_engine.try_lock() {
            for err in engine.drain_pending_errors() {
                window.push_notification(
                    Notification::error(t!("Notification.Title.MeshLoadFailed").to_string())
                        .message(err),
                    cx,
                );
            }
        }

        // Capture a project thumbnail if a save just requested one.
        // This must happen synchronously since it reads back GPU data.
        let capture_path = self
            .shared_state
            .write()
            .build
            .pending_thumbnail_capture
            .take();
        if let Some(path) = capture_path {
            if let Some(ref surface) = self.surface {
                if let Some((view, (w, h))) = surface.back_view_with_size() {
                    if let Ok(mut engine) = self.gpu_engine.try_lock() {
                        capture_viewport_thumbnail(&mut engine, surface, w, h, format, &path);
                    }
                }
            }
        }

        // Build the viewport element
        let viewport_element = if let Some(surface) = self.surface.clone() {
            // Repaint this view whenever the render thread publishes a new frame,
            // so the compositor promotes `ready` -> `display` and
            // `WgpuSurface::prepaint` keeps observing the element's bounds (which
            // is what resizes the surface when the viewport panel changes size).
            //
            // The pump runs on `Window::on_next_frame`, which fires every platform
            // frame *without* marking anything dirty, so idle frames now cost
            // nothing instead of rebuilding the whole editor chrome.
            self.start_frame_pump(window, cx);
            // This render paints whatever the compositor promotes, so the pump's
            // outstanding request is satisfied and it may fire again. Prepaint of
            // the surface element below is also what re-observes bounds, so the
            // periodic-full-render counter restarts here.
            self.awaiting_render = false;
            self.frames_since_full_render = 0;
            self.viewport_size_at_last_full_render = Some(window.viewport_size());
            self.last_published_frame = self.frames_published.load(Ordering::Acquire);

            wgpu_surface(surface)
                .defer_resize_until_mouse_up(true)
                .absolute()
                .inset_0()
                .into_any_element()
        } else {
            // Surface not created yet: render nothing rather than a placeholder.
            // This lasts a single frame, so any overlay just reads as a flash.
            div()
                .relative()
                .track_focus(&self.focus_handle)
                .id("helio_viewport")
                .size_full()
                .into_any_element()
        };

        // Accept mesh/scene/blueprint payload drags and forward successful drops to the viewport entity.
        let viewport = cx.entity().clone();
        div()
            .id("helio-viewport-drop")
            .size_full()
            .drag_over::<AssetPayload>(|style, payload, _window, cx| {
                if matches!(
                    payload.kind,
                    AssetKind::Mesh | AssetKind::Scene | AssetKind::Blueprint
                ) {
                    style
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded(px(4.0))
                } else {
                    style.opacity(0.4)
                }
            })
            .on_drop::<AssetPayload>(move |payload, window, cx| {
                if matches!(
                    payload.kind,
                    AssetKind::Mesh | AssetKind::Scene | AssetKind::Blueprint
                ) {
                    let payload = payload.clone();
                    viewport.update(cx, |this, cx| {
                        this.handle_asset_drop(&payload, window, cx);
                    });
                }
            })
            .child(viewport_element)
            .into_any_element()
    }
}
