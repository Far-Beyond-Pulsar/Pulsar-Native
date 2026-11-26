//! The steps of drawing one frame: scene step, frame preparation, submission.

use super::*;

impl HelioRenderer {
    /// Resize the Helio renderer in place to the new surface size.
    pub(super) fn resize_renderer(
        &mut self,
        inner: &mut HelioInner,
        surface: SurfaceSpec,
        scene_revision: u64,
    ) {
        let SurfaceSpec { width, height, .. } = surface;
        frame_span!("pulsar: HelioRenderer::resize");
        frame_diagnostic!((width, height, scene_revision, self.frame_count));
        profiling::profile_scope!("helio_resize");
        tracing::debug!(
            from = ?self.viewport_size,
            to = ?(width, height),
            "Resizing Helio renderer in place"
        );
        inner.scene.renderer.set_render_size(width, height);
        self.viewport_size = (width, height);
    }

    /// Apply the deselect and gizmo-mode commands the UI queued.
    pub(super) fn apply_pending_editor_commands(&mut self, inner: &mut HelioInner) {
        if self.pending_deselect.swap(false, Ordering::AcqRel) {
            self.scene_store.write().world.select(None);
            inner.interaction.cancel_drag();
            self.gizmo_dirty = true;
        }
        if let Ok(mut pending) = self.pending_gizmo_mode.lock() {
            if let Some(mode) = pending.take() {
                inner.interaction.set_mode(mode);
                self.gizmo_dirty = true;
            }
        }
    }

    /// Finish a frame that draws nothing: idle frames still serve inspector
    /// requests and report their metrics.
    pub(super) fn publish_idle_frame(&mut self, dt: f32) {
        {
            profiling::profile_scope!("helio_idle_publish_inspector_snapshot");
            self.scene_store.read().world.publish_inspector_snapshot();
        }
        self.update_frame_metrics(dt);
    }

    /// Publish the frame rate and count to the shared metrics.
    pub(super) fn update_frame_metrics(&self, dt: f32) {
        if let Ok(mut m) = self.metrics.lock() {
            m.fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
            m.frame_time_ms = dt * 1000.0;
            m.frames_rendered = self.frame_count;
        }
    }

    /// Step SceneDB and the plugins' editor line sets when the scene changed.
    /// Returns the time spent in milliseconds.
    ///
    /// SceneDB owns all world state and performs its own GPU-mirror flush.
    /// The renderer only advances that authoritative database at the frame
    /// boundary; it never builds a CPU projection or submits per-object data.
    /// Content still settling steps too: its scene subsystems take what
    /// finished off the thread (a voxel world built, a pick answered).
    pub(super) fn step_scene(
        &mut self,
        inner: &mut HelioInner,
        steps_scene: bool,
        has_pending_scene: bool,
    ) -> f64 {
        let mut sync_ms = 0.0;
        if steps_scene && (has_pending_scene || inner.scene.content_settling()) {
            frame_span!("pulsar: HelioRenderer::scene_db_step");
            profiling::profile_scope!("helio_scene_db_step");
            let t_sync = Instant::now();
            // Exclusive lock: contends with the UI thread (inspector, property
            // edits, hierarchy) for as long as either side holds the scene.
            let mut scene_store = {
                profiling::profile_scope!("helio_scene_store_write_lock_wait");
                self.scene_store.write()
            };
            {
                // The plugins' editor line sets (a spline's curve) are drawn
                // by Helio's editor debug pass in world space, so they follow
                // the camera like the grid; each rebuilds only when what it
                // shows changed.
                profiling::profile_scope!("helio_sync_editor_lines");
                let editor_lines = inner.editor_lines.get_or_insert_with(|| {
                    super::super::render_features::editor_lines(&scene_store.world)
                });
                for (name, lines) in editor_lines.iter_mut() {
                    if let Some(lines) = lines.poll(&scene_store.world) {
                        inner.scene.renderer.debug_set_editor_lines(name, lines);
                    }
                }
            }
            // SceneDB steps, with the subsystems plugins add to scenes
            // (voxel worlds build and follow their journals).
            inner.scene.step_scene(&mut scene_store);
            sync_ms = t_sync.elapsed().as_secs_f64() * 1000.0;
            inner.last_scene_revision = scene_store.world.revision();
        }
        sync_ms
    }

    /// Read the ground under the camera: its frame follows the ground's local
    /// vertical, and its speed the height above it.
    pub(super) fn track_camera_ground(&mut self, inner: &mut HelioInner, phases: &mut FramePhases) {
        let ground = inner.scene.camera_ground(self.cam.position);
        self.camera_ground = ground;
        phases.mark("ground");
        if ground.up.is_some() || !self.active_space_traversal {
            self.cam.follow_ground(ground.up);
        }
    }

    /// Build the camera and publish the scene's content for this frame.
    pub(super) fn prepare_scene_frame(
        &mut self,
        inner: &mut HelioInner,
        req: &FrameRequest<'_>,
        ortho_half_height: f32,
    ) -> PreparedFrame {
        let SurfaceSpec { width, height, .. } = req.surface;
        let crop = req.crop;
        let (forward, frame_up) = match self.active_view.axis_basis() {
            Some((forward, _, up)) => (forward, up),
            None => (self.cam.forward(), self.cam.frame_up()),
        };
        frame_span!("pulsar: HelioRenderer::frame_prepare");
        inner.scene.prepare_frame(
            &FrameView {
                eye: self.cam.position,
                coordinate_space: (!self.active_view.is_orthographic()
                    && self.active_space_traversal)
                    .then_some(self.camera_space),
                forward,
                up: frame_up,
                aspect: crop.map_or(width as f32 / height.max(1) as f32, |crop| {
                    crop.full_size[0] as f32 / crop.full_size[1].max(1) as f32
                }),
                projection: if self.active_view.is_orthographic() {
                    FrameProjection::Orthographic {
                        half_height: ortho_half_height,
                    }
                } else {
                    FrameProjection::Perspective {
                        fov_y: std::f32::consts::FRAC_PI_4,
                        near: 0.1,
                        far: 10_000.0,
                    }
                },
                crop,
            },
            [width, height],
        )
    }

    /// Clear last frame's debug geometry and draw the gizmo.
    ///
    /// Debug geometry is transient GPU execution state. World content is
    /// read by Helio passes directly from the SceneDB GPU mirror.
    pub(super) fn draw_editor_debug(&self, inner: &mut HelioInner) {
        {
            profiling::profile_scope!("helio_debug_clear");
            inner.scene.renderer.debug_clear();
        }
        let store = {
            profiling::profile_scope!("helio_scene_store_read_lock_wait (gizmo)");
            self.scene_store.read()
        };
        if self.capture_view.hide_gizmos {
            return;
        }
        profiling::profile_scope!("helio_draw_gizmo");
        inner
            .interaction
            .draw_gizmo(&mut inner.scene.renderer, &store.world);
    }

    /// Reset TSR history when a capture tile or a camera jump asked for it.
    pub(super) fn reset_taa_history_if_requested(&mut self, inner: &mut HelioInner, capture: bool) {
        if capture {
            self.reset_taa_next_frame = true;
        }
        if self.reset_taa_next_frame {
            if let Some(pass) = inner
                .scene
                .renderer
                .find_pass_mut::<helio_pass_tsr::TsrPass>()
            {
                pass.reset_history();
            }
            self.reset_taa_next_frame = false;
        }
    }

    /// Log the periodic `VOXEL_STATS` line when stats logging is on.
    pub(super) fn log_voxel_stats(&mut self, inner: &HelioInner, width: u32, height: u32) {
        if self.stats_log && self.last_stats_log.elapsed().as_secs_f32() >= 0.5 {
            let fps = self.stats_frames.0 as f32 / self.last_stats_log.elapsed().as_secs_f32();
            let slowest = self.stats_frames.1;
            self.stats_frames = (0, 0.0);
            self.last_stats_log = Instant::now();
            let (forward, _, up) = self.cam.basis();
            for line in inner.scene.diagnostics() {
                tracing::info!(
                    "VOXEL_STATS fps={fps:.1} slowest_frame_ms={slowest:.1} altitude={:.1} speed_scale={:.1} {line} eye={:?} forward={:?} up={:?} viewport={}x{} configured_render_scale={:.2} graph_gpu_ms={:?}",
                    self.camera_ground.altitude.unwrap_or(f64::NAN),
                    altitude_speed_scale(&self.camera_ground),
                    self.cam.position.to_array(),
                    forward.to_array(),
                    up.to_array(),
                    width,
                    height,
                    inner.scene.renderer.render_scale(),
                    inner.scene.renderer.gpu_frame_ms(),
                );
            }
        }
    }

    /// Upload, render the graph into the target and return its submission
    /// plus whether the render succeeded.
    pub(super) fn submit_frame(
        &mut self,
        inner: &mut HelioInner,
        req: &FrameRequest<'_>,
        camera: &helio::Camera,
        realtime: bool,
        phases: &mut FramePhases,
    ) -> (Option<wgpu::SubmissionIndex>, bool) {
        let crop = req.crop;
        let mut render_succeeded = false;
        frame_span!("pulsar: HelioRenderer::render_submit");
        profiling::profile_scope!("helio_render_submit");
        {
            profiling::profile_scope!("helio_upload");
            inner.scene.upload();
        }
        // Separates a wait on the scene lock (the UI thread editing)
        // from the graph's own work in VOXEL_FRAME_PHASES.
        phases.mark("flush");
        {
            profiling::profile_scope!("helio_renderer_render");
            let renderer = &mut inner.scene.renderer;
            // Keep temporal-filter weights deterministic across tiles.
            // Animation time is frozen independently through the frame
            // clock override below; Helio requires a positive delta for
            // temporal filters even while that clock is paused.
            renderer.set_frame_delta_override(if crop.is_some() {
                Some(1.0 / 60.0)
            } else {
                self.frame_delta_override
            });
            renderer.set_camera_jitter_override(if crop.is_some() {
                Some([0.0, 0.0])
            } else {
                self.camera_jitter_override
            });
            renderer.set_frame_clock_delta(if realtime { None } else { Some(0.0) });
            match renderer.render(camera, req.view) {
                Ok(()) => render_succeeded = true,
                Err(e) => tracing::error!("Helio render error: {:?}", e),
            }
        }
        // The graph's own submission carries the frame. Only when Helio
        // submitted nothing (an error) fall back to an empty submit, which
        // takes the queue lock and can drain pending work.
        let submission = inner.scene.renderer.last_submission().or_else(|| {
            profiling::profile_scope!("helio_queue_submit (fence)");
            Some(
                inner
                    .queue
                    .submit(std::iter::empty::<wgpu::CommandBuffer>()),
            )
        });
        (submission, render_succeeded)
    }
}
