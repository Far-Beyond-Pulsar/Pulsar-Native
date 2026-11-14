//! GPU renderer service — thin wrapper around HelioRenderer.
//!
//! The lightweight renderer controller is created synchronously by the builder.
//! Helio's render graph and wgpu resources are initialized lazily once the
//! `WgpuSurface` is available. The editor prepares them on its dedicated
//! `Helio Render` thread before taking WGPUI's GPU submit guard for its first
//! frame, so initial graph construction, pipeline creation, and cache loading
//! cannot hold up a surface reconfiguration.

use crate::subsystems::render::{
    EditorCameraState, FrameCrop, HelioRenderer, RenderMetrics, RenderSpikeLogConfig,
};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

/// Builder for `GpuRenderer`.
pub struct GpuRendererBuilder {
    scene_store: Option<crate::scene::SharedScene>,
    construct_pump: Option<crate::scene::EditorConstructPump>,
}

impl GpuRendererBuilder {
    pub fn new(_width: u32, _height: u32) -> Self {
        Self {
            scene_store: None,
            construct_pump: None,
        }
    }

    pub fn scene_db(mut self, store: crate::scene::SharedScene) -> Self {
        self.scene_store = Some(store);
        self
    }

    /// The editor's `construct` pump, run once per rendered frame
    /// (`crate::scene::construct_pump`).
    pub fn construct_pump(mut self, pump: crate::scene::EditorConstructPump) -> Self {
        self.construct_pump = Some(pump);
        self
    }

    pub fn build(self) -> GpuRenderer {
        let scene_store = self
            .scene_store
            .unwrap_or_else(|| Arc::new(RwLock::new(crate::scene::new_scene())));
        GpuRenderer {
            helio_renderer: Some({
                let mut renderer = HelioRenderer::new(scene_store);
                if let Some(pump) = self.construct_pump {
                    renderer.set_construct_pump(pump);
                }
                renderer
            }),
            frame_count: 0,
            start_time: Instant::now(),
            fps_window_start: Instant::now(),
            fps_window_frames: 0,
            fps: 0.0,
            stats_feed: RendererStatsFeed::default(),
            details_published: None,
            gpu_memory: None,
        }
    }
}

/// Frames the renderer submitted, counted on every one. Two reads give exact
/// rates: frames per second is the change in `frames` over the change in
/// `last`, and the CPU cost of a frame is the change in `busy` over the change
/// in `frames`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RenderTally {
    /// Frames submitted to the GPU (an idle frame that drew nothing is not one).
    pub frames: u64,
    /// When the latest of them finished recording.
    pub last: Option<Instant>,
    /// CPU time spent recording and submitting them, in total.
    pub busy: std::time::Duration,
}

/// What the renderer publishes less often than every frame.
#[derive(Debug, Clone, Default)]
pub struct RendererDetails {
    /// Per-pass CPU and GPU timings, and the GPU frame total.
    pub profiler: Option<crate::subsystems::render::GpuProfilerData>,
    /// Bytes the device's allocator holds (buffers and textures), when the
    /// backend reports it.
    pub gpu_memory_bytes: Option<u64>,
}

/// The renderer's statistics, for diagnostics overlays.
#[derive(Debug, Clone, Default)]
pub struct RendererStats {
    pub tally: RenderTally,
    /// `None` before the first frame.
    pub details: Option<Arc<RendererDetails>>,
}

/// Where [`GpuRenderer`] publishes [`RendererStats`].
///
/// The render scheduler holds the renderer's mutex for the whole of every
/// frame, so a reader that `try_lock`s the renderer misses most of the time
/// and a diagnostics overlay polling it freezes. The feed is the renderer's
/// statistics behind their own lock, held only to copy the tally or swap an
/// `Arc`: take the handle once ([`GpuRenderer::stats_feed`]) and read it on
/// any thread without touching the renderer.
#[derive(Debug, Clone, Default)]
pub struct RendererStatsFeed(Arc<parking_lot::Mutex<RendererStats>>);

impl RendererStatsFeed {
    /// The statistics as of now.
    pub fn latest(&self) -> RendererStats {
        self.0.lock().clone()
    }

    fn count_frame(&self, busy: std::time::Duration) {
        let mut stats = self.0.lock();
        stats.tally.frames += 1;
        stats.tally.last = Some(Instant::now());
        stats.tally.busy += busy;
    }

    fn publish_details(&self, details: RendererDetails) {
        self.0.lock().details = Some(Arc::new(details));
    }
}

/// How often the renderer publishes [`RendererDetails`]. Overlays show them at
/// about 10 Hz; this keeps the profiler clone off most frames.
const DETAILS_PUBLISH: std::time::Duration = std::time::Duration::from_millis(50);

/// How often the allocator report behind
/// [`RendererDetails::gpu_memory_bytes`] is generated: it walks every
/// allocation.
const GPU_MEMORY_REFRESH: std::time::Duration = std::time::Duration::from_secs(1);

/// GPU renderer — drives Helio through a GPUI `WgpuSurfaceHandle`.
///
/// This is the **only** public interface to the renderer subsystem.
/// Callers must not access `helio_renderer` directly; use the methods below.
pub struct GpuRenderer {
    helio_renderer: Option<HelioRenderer>,
    frame_count: u64,
    start_time: Instant,
    // Rolling-window frame rate. `frame_count / start_time.elapsed()` is a
    // lifetime average: it converges to the mean over the whole process and
    // barely moves in response to a change, which makes it useless for judging
    // whether a pacing or caching change did anything. These three fields
    // recompute the rate over a short window instead.
    fps_window_start: Instant,
    fps_window_frames: u64,
    fps: f32,
    stats_feed: RendererStatsFeed,
    details_published: Option<Instant>,
    gpu_memory: Option<(Instant, Option<u64>)>,
}

/// Length of the rolling window used to compute [`GpuRenderer::get_fps`].
/// Short enough to react within a fraction of a second, long enough that the
/// number isn't visibly jittery.
const FPS_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

impl GpuRenderer {
    /// Prepare Helio's graph before acquiring a surface submit guard. This is
    /// the expensive first-frame path and can compile many pipelines.
    pub fn prepare_for_render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> bool {
        self.helio_renderer
            .as_mut()
            .map(|renderer| renderer.prepare_for_render(device, queue, width, height, format))
            .unwrap_or(false)
    }

    /// Free the Helio renderer's GPU resources (its graph and passes); the
    /// scene, camera and settings stay, and the next
    /// [`prepare_for_render`](Self::prepare_for_render) builds them again. A
    /// stopped render runtime calls it.
    pub fn release_gpu_resources(&mut self) {
        if let Some(renderer) = self.helio_renderer.as_mut() {
            renderer.release_gpu_resources();
        }
    }

    /// Estimated bytes of the textures the current render graph allocates,
    /// once the renderer is built.
    pub fn graph_texture_bytes(&self) -> Option<u64> {
        self.helio_renderer
            .as_ref()
            .and_then(|renderer| renderer.graph_texture_bytes())
    }

    /// Render one frame directly into a `WgpuSurfaceHandle` back-buffer view.
    pub fn render_frame_to_surface(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Option<wgpu::SubmissionIndex> {
        self.render_frame_to_surface_inner(device, queue, view, width, height, format, None)
    }

    /// Render one frozen time-slice tile for an oversized screenshot.
    pub fn render_capture_tile(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        crop: FrameCrop,
    ) -> Option<wgpu::SubmissionIndex> {
        self.render_frame_to_surface_inner(device, queue, view, width, height, format, Some(crop))
    }

    fn render_frame_to_surface_inner(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        crop: Option<FrameCrop>,
    ) -> Option<wgpu::SubmissionIndex> {
        profiling::profile_scope!("GpuRenderer::render_frame_to_surface");
        let started = Instant::now();
        let result = if let Some(ref mut r) = self.helio_renderer {
            if let Some(crop) = crop {
                r.render_capture_tile(device, queue, view, width, height, format, crop)
            } else {
                r.render_frame(device, queue, view, width, height, format)
            }
        } else {
            None
        };
        self.frame_count += 1;

        self.fps_window_frames += 1;
        let window_elapsed = self.fps_window_start.elapsed();
        if window_elapsed >= FPS_WINDOW {
            self.fps = self.fps_window_frames as f32 / window_elapsed.as_secs_f32();
            self.fps_window_start = Instant::now();
            self.fps_window_frames = 0;
        }

        // A screenshot tile is not a frame of the view.
        if result.is_some() && crop.is_none() {
            self.stats_feed.count_frame(started.elapsed());
            self.publish_details(device);
        }

        result
    }

    /// Publish the profiler and GPU memory to the stats feed, at most every
    /// [`DETAILS_PUBLISH`] (memory every [`GPU_MEMORY_REFRESH`]).
    fn publish_details(&mut self, device: &wgpu::Device) {
        let now = Instant::now();
        if self
            .details_published
            .is_some_and(|at| now - at < DETAILS_PUBLISH)
        {
            return;
        }
        self.details_published = Some(now);
        let gpu_memory_bytes = match self.gpu_memory {
            Some((at, bytes)) if now - at < GPU_MEMORY_REFRESH => bytes,
            _ => {
                profiling::profile_scope!("GpuRenderer: allocator report");
                let bytes = device
                    .generate_allocator_report()
                    .map(|report| report.total_allocated_bytes);
                self.gpu_memory = Some((now, bytes));
                bytes
            }
        };
        self.stats_feed.publish_details(RendererDetails {
            profiler: self.get_gpu_profiler_data(),
            gpu_memory_bytes,
        });
    }

    /// How long a frame should take: its output's refresh interval (`None`:
    /// unpaced). See `HelioRenderer::set_frame_budget`.
    pub fn set_frame_budget(&mut self, budget: Option<std::time::Duration>) {
        if let Some(renderer) = self.helio_renderer.as_mut() {
            renderer.set_frame_budget(budget);
        }
    }

    /// The handle the renderer publishes its statistics to. Take it once and
    /// read it without locking the renderer (see [`RendererStatsFeed`]).
    pub fn stats_feed(&self) -> RendererStatsFeed {
        self.stats_feed.clone()
    }

    /// Frames per second submitted by the renderer, averaged over the last
    /// [`FPS_WINDOW`].
    ///
    /// Note this counts *renderer* frames — one per `render_frame_to_surface`
    /// call on the Helio render thread — not GPUI UI frames. The two are not
    /// the same: the compositor promotes at most one renderer frame per UI
    /// draw, and UI draws are skipped entirely when nothing is dirty.
    pub fn get_fps(&self) -> f32 {
        if self.fps > 0.0 {
            self.fps
        } else {
            self.get_helio_fps()
        }
    }

    pub fn get_helio_fps(&self) -> f32 {
        self.helio_renderer
            .as_ref()
            .map(|r| r.get_metrics().fps)
            .unwrap_or(0.0)
    }

    pub fn get_render_fps(&self) -> f32 {
        self.get_fps().max(self.get_helio_fps())
    }

    /// Whether the next [`prepare_for_render`](Self::prepare_for_render) builds
    /// a graph (the first, a new pipeline, changed graph settings).
    pub fn needs_build(&mut self) -> bool {
        self.helio_renderer
            .as_mut()
            .is_some_and(|renderer| renderer.needs_build())
    }

    pub fn is_initialized(&self) -> bool {
        self.helio_renderer
            .as_ref()
            .map(|r| r.is_initialized())
            .unwrap_or(false)
    }

    pub fn get_render_metrics(&self) -> Option<RenderMetrics> {
        self.helio_renderer.as_ref().map(|r| r.get_metrics())
    }

    pub fn get_gpu_profiler_data(&self) -> Option<crate::subsystems::render::GpuProfilerData> {
        self.helio_renderer
            .as_ref()
            .map(|r| r.get_gpu_profiler_data())
    }

    pub fn set_spike_log_config(&mut self, config: RenderSpikeLogConfig) {
        if let Some(renderer) = self.helio_renderer.as_mut() {
            renderer.set_spike_log_config(config);
        }
    }

    pub fn spike_log_config(&self) -> Option<RenderSpikeLogConfig> {
        self.helio_renderer
            .as_ref()
            .map(HelioRenderer::spike_log_config)
    }

    pub fn get_frame_count(&self) -> u64 {
        self.frame_count
    }

    // ── Gizmo and selection API ───────────────────────────────────────────────

    /// Drain all pending mesh-load error messages accumulated since the last call.
    /// Returns the drained messages so the UI can display notifications.
    pub fn drain_pending_errors(&self) -> Vec<String> {
        self.helio_renderer
            .as_ref()
            .and_then(|r| r.pending_errors.lock().ok())
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }

    /// Camera input handle for the viewport input thread.
    pub fn camera_input(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<crate::subsystems::render::CameraInput>>> {
        self.helio_renderer.as_ref().map(|r| r.camera_input.clone())
    }

    /// Pointer-event (left-click/left-release) queue handle for the viewport
    /// input thread, fetched once alongside `camera_input()`. Pushing onto
    /// this instead of calling `handle_left_click`/`handle_left_release`
    /// through `gpu_engine` removes the `try_lock()` race that could
    /// silently drop a release event and permanently wedge `is_dragging()`
    /// -- see `PendingPointerEvent`'s doc.
    pub fn pointer_event_queue(
        &self,
    ) -> Option<
        std::sync::Arc<
            std::sync::Mutex<Vec<crate::subsystems::render::helio_renderer::PendingPointerEvent>>,
        >,
    > {
        self.helio_renderer
            .as_ref()
            .map(|r| r.pending_pointer_events.clone())
    }

    /// Cheap, lock-free-of-`gpu_engine` handle bundle for one-shot editor
    /// commands (gizmo mode, deselect) -- see
    /// `HelioEditorMailbox`'s doc. Fetched once at panel construction.
    pub fn editor_mailbox(
        &self,
    ) -> Option<crate::subsystems::render::helio_renderer::HelioEditorMailbox> {
        self.helio_renderer.as_ref().map(|r| r.editor_mailbox())
    }

    pub fn editor_camera_state(&self) -> Option<EditorCameraState> {
        self.helio_renderer
            .as_ref()
            .map(|r| r.editor_camera_state())
    }

    /// Resolve a normalized viewport pixel to the editor ground plane.
    pub fn editor_placement_position(
        &self,
        norm_x: f32,
        norm_y: f32,
        ignore_id: Option<&str>,
    ) -> Option<[f64; 3]> {
        self.helio_renderer
            .as_ref()
            .and_then(|renderer| renderer.editor_placement_position(norm_x, norm_y, ignore_id))
    }

    pub fn set_editor_camera_state(&mut self, state: EditorCameraState) {
        if let Some(r) = self.helio_renderer.as_mut() {
            r.set_editor_camera_state(state);
        }
    }

    /// Queue a new Helio gizmo mode for the next render frame.
    pub fn queue_gizmo_mode(&self, mode: crate::GizmoMode) {
        if let Some(r) = &self.helio_renderer {
            r.queue_gizmo_mode(mode);
        }
    }

    /// Request deselection at the start of the next render frame.
    pub fn queue_deselect(&self) {
        if let Some(r) = &self.helio_renderer {
            r.queue_deselect();
        }
    }

    /// Get the ID of the currently selected object in SceneDb.
    pub fn get_scene_db_selected_id(&self) -> Option<String> {
        self.helio_renderer
            .as_ref()
            .and_then(|r| r.get_scene_db_selected_id())
    }

    /// Get the ID of the object currently selected inside the Helio editor state
    /// (set by viewport click/gizmo; may lag one frame behind SceneDb selection).
    pub fn get_helio_selected_scene_db_id(&self) -> Option<String> {
        self.helio_renderer
            .as_ref()
            .and_then(|r| r.get_selected_scene_db_id())
    }

    /// Sync the SceneDb selection into Helio's editor state so the gizmo appears.
    pub fn sync_selection_to_helio(&mut self) {
        if let Some(r) = &mut self.helio_renderer {
            let scene_selected = r.get_scene_db_selected_id();
            let helio_selected = r.get_selected_scene_db_id();
            if scene_selected != helio_selected {
                match &scene_selected {
                    Some(id) => {
                        r.select_by_scene_db_id(id);
                    }
                    None => {
                        r.deselect();
                    }
                }
            }
        }
    }

    // ── Mouse / viewport input ────────────────────────────────────────────────

    /// Forward a normalized mouse-move event to the Helio editor state (gizmo drag).
    pub fn handle_mouse_move(&mut self, norm_x: f32, norm_y: f32) {
        if let Some(r) = &mut self.helio_renderer {
            r.handle_mouse_move(norm_x, norm_y);
        }
    }

    /// Forward a normalized left-click to the Helio scene picker.
    pub fn handle_left_click(&mut self, norm_x: f32, norm_y: f32) {
        if let Some(r) = &mut self.helio_renderer {
            r.handle_left_click(norm_x, norm_y);
        }
    }

    /// Signal the end of a left-button drag (finalises gizmo transform).
    pub fn handle_left_release(&mut self) {
        if let Some(r) = &mut self.helio_renderer {
            r.handle_left_release();
        }
    }

    /// Request TAA history reset on the next rendered frame.
    pub fn reset_taa(&mut self) {
        if let Some(r) = &mut self.helio_renderer {
            r.reset_taa();
        }
    }

    /// Send a fire-and-forget command to the renderer thread (e.g. ToggleFeature).
    pub fn send_renderer_command(
        &self,
        cmd: crate::subsystems::render::helio_renderer::RendererCommand,
    ) {
        if let Some(r) = &self.helio_renderer {
            let _ = r.command_sender.send(cmd);
        }
    }
}

// SAFETY: the compiler cannot prove this; it holds by how the renderer is used.
// The non-`Send` parts (the plugins' render-host parts and scene derivations as
// `dyn` objects, the legacy command `Receiver`) are reached only through
// `&mut self` methods, so the owner has exclusive access to them, and the
// renderer is always held as `Arc<Mutex<GpuRenderer>>` by its owners (the
// level editor), which serialises those calls. The renderer is moved between
// threads whole and its trait objects are thread-agnostic (they hold no
// thread-local or thread-affine state).
unsafe impl Send for GpuRenderer {}
// SAFETY: the `&self` methods touch only `Arc<Mutex<_>>`/atomic mailbox state
// and the command `Sender`, which are `Sync`; everything else needs `&mut self`
// (see `Send` above), so shared references cannot reach the non-`Sync` parts.
unsafe impl Sync for GpuRenderer {}

#[cfg(test)]
mod tests {
    use super::GpuRendererBuilder;
    use crate::subsystems::render::RenderSpikeLogConfig;
    use std::time::Duration;

    #[test]
    fn spike_log_config_is_available_through_the_public_renderer_service() {
        let mut renderer = GpuRendererBuilder::new(1, 1).build();
        let config = RenderSpikeLogConfig {
            enabled: true,
            cpu_threshold_ms: 18.0,
            gpu_threshold_ms: 12.0,
            min_interval: Duration::from_millis(125),
        };

        renderer.set_spike_log_config(config);

        assert_eq!(renderer.spike_log_config(), Some(config));
    }

    // ── Pulsar-Native drag-release freeze fix: mailbox plumbing ────────────
    // These exercise the queue/mailbox data structures in isolation, with no
    // render thread involved -- the actual drag-release scenario itself
    // still needs manual verification in the running editor (a real render
    // thread racing real GPUI mouse events), which no automated test here
    // can reach.

    #[test]
    fn pointer_event_queue_is_available_right_after_construction() {
        let renderer = GpuRendererBuilder::new(1, 1).build();
        assert!(
            renderer.pointer_event_queue().is_some(),
            "GpuRendererBuilder::build always sets helio_renderer synchronously"
        );
    }

    #[test]
    fn queue_left_click_and_release_push_onto_the_queue_in_order() {
        use crate::subsystems::render::PendingPointerEvent;

        let renderer = GpuRendererBuilder::new(1, 1).build();
        let queue = renderer
            .pointer_event_queue()
            .expect("pointer event queue should exist");

        // Simulate what viewport/mod.rs's on_mouse_down/on_mouse_up closures
        // do -- push directly onto the shared queue, no gpu_engine lock.
        queue
            .lock()
            .expect("pointer event queue lock poisoned")
            .push(PendingPointerEvent::LeftClick {
                norm_x: 0.25,
                norm_y: 0.75,
            });
        queue
            .lock()
            .expect("pointer event queue lock poisoned")
            .push(PendingPointerEvent::LeftRelease);

        let drained = std::mem::take(&mut *queue.lock().unwrap());
        assert_eq!(drained.len(), 2);
        match drained[0] {
            PendingPointerEvent::MouseMove { .. } | PendingPointerEvent::ToolRay { .. } => {
                panic!("expected LeftClick first")
            }
            PendingPointerEvent::LeftClick { norm_x, norm_y } => {
                assert_eq!((norm_x, norm_y), (0.25, 0.75));
            }
            PendingPointerEvent::LeftRelease => panic!("expected LeftClick first"),
        }
        assert!(matches!(drained[1], PendingPointerEvent::LeftRelease));

        // Queue is empty again after the drain -- matches render_frame's own
        // std::mem::take drain, proving a second drain sees nothing stale.
        assert!(queue.lock().unwrap().is_empty());
    }

    #[test]
    fn drag_samples_and_hover_moves_coalesce_while_clicks_keep_order() {
        use crate::subsystems::render::{PendingPointerEvent as E, ToolRay};
        let tool = ToolRay(std::sync::Arc::new(|_, _, _, _| {}));
        let brush = |x: f32, start: bool| E::ToolRay {
            norm_x: x,
            norm_y: 0.5,
            start,
            tool: tool.clone(),
        };
        let mut queue = Vec::new();
        E::queue(&mut queue, brush(0.0, true));
        // A high-Hz drag between two frames: hover moves and brush samples
        // interleave.
        for n in 1..=50 {
            E::queue(
                &mut queue,
                E::MouseMove {
                    norm_x: n as f32,
                    norm_y: 0.5,
                },
            );
            E::queue(&mut queue, brush(n as f32, false));
        }
        assert_eq!(queue.len(), 3, "{queue:?}");
        assert!(matches!(queue[0], E::ToolRay { start: true, norm_x, .. } if norm_x == 0.0));
        assert!(matches!(queue[1], E::MouseMove { norm_x, .. } if norm_x == 50.0));
        assert!(matches!(queue[2], E::ToolRay { start: false, norm_x, .. } if norm_x == 50.0));
        // Nothing latest-wins jumps a release or a stroke start.
        E::queue(&mut queue, E::LeftRelease);
        E::queue(&mut queue, brush(60.0, true));
        E::queue(&mut queue, brush(61.0, false));
        E::queue(
            &mut queue,
            E::MouseMove {
                norm_x: 61.0,
                norm_y: 0.5,
            },
        );
        assert_eq!(queue.len(), 7, "{queue:?}");
        assert!(matches!(queue[3], E::LeftRelease));
        assert!(matches!(queue[5], E::ToolRay { start: false, norm_x, .. } if norm_x == 61.0));
    }

    #[test]
    fn editor_mailbox_queue_deselect_sets_its_flag() {
        use std::sync::atomic::Ordering;

        let renderer = GpuRendererBuilder::new(1, 1).build();
        let mailbox = renderer
            .editor_mailbox()
            .expect("editor mailbox should exist");

        mailbox.queue_deselect();

        // `tests` is a child module of `gpu_renderer`, so it can reach
        // `GpuRenderer`'s private `helio_renderer` field directly to prove
        // the mailbox's writes actually land on the same `HelioRenderer`
        // `render_frame` itself reads from -- `HelioEditorMailbox::Clone` is
        // a shallow `Arc` handle clone, not a deep copy, so this is the same
        // underlying `AtomicBool`s the render thread would see.
        let inner = renderer.helio_renderer.as_ref().unwrap();
        assert!(inner.pending_deselect.load(Ordering::Acquire));
    }
}
