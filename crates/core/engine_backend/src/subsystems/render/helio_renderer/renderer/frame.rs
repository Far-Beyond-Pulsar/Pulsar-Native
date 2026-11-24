//! One editor frame: input and camera updates, then the draw.

use super::*;

impl HelioRenderer {
    /// Called each GPUI frame from the viewport.
    pub fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Option<wgpu::SubmissionIndex> {
        self.render_frame_internal(FrameRequest {
            device,
            queue,
            view,
            surface: SurfaceSpec {
                width,
                height,
                format,
            },
            crop: None,
        })
    }

    /// Draw one crop of a larger output while keeping scene time fixed.
    #[allow(clippy::too_many_arguments)] // the device and queue, the target, and the crop's geometry
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
        self.render_frame_internal(FrameRequest {
            device,
            queue,
            view,
            surface: SurfaceSpec {
                width,
                height,
                format,
            },
            crop: Some(crop),
        })
    }

    /// Run one editor frame: input and camera updates, the idle check, then
    /// scene sync, frame preparation, submission and profiler reporting.
    /// Returns `None` when nothing was drawn.
    pub(super) fn render_frame_internal(
        &mut self,
        req: FrameRequest<'_>,
    ) -> Option<wgpu::SubmissionIndex> {
        // When the Inspector capture is active, put the engine-side render
        // phases into the same bounded WGPUI timeline as Window::draw.
        frame_profile_bridge!();
        frame_span!("pulsar: HelioRenderer::render_frame");
        profiling::profile_scope!("helio_frame");
        let frame_start = Instant::now();
        let mut phases = FramePhases::new(frame_start);
        let clock = self.tick_frame_clock(frame_start, req.crop.is_some());
        let SurfaceSpec {
            width,
            height,
            format,
        } = req.surface;

        if !self.prepare_for_render(req.device, req.queue, width, height, format) {
            return None;
        }
        let previous_viewport_size = self.viewport_size;
        self.viewport_size = (width, height);
        self.configure_gizmo_view();
        self.apply_frame_start_gizmo_state();

        self.process_pending_pointer_events(req.crop.is_some());
        phases.mark("pointer");

        let needs_resize = self.poll_camera_input(req.crop.is_some())?;
        frame_diagnostic!((self.frame_count, width, height));

        let camera_update = self.update_camera(clock.dt, req.crop.is_some());
        phases.mark("camera");
        self.configure_gizmo_view();
        self.viewport_size = previous_viewport_size;

        let editor_state = self.apply_editor_baselines();
        let mut changes = FrameChanges {
            needs_resize,
            view_kind: camera_update.view_kind_changed,
            camera_synced: camera_update.camera_synced,
            postprocess: editor_state.postprocess_changed,
            debug_view: editor_state.debug_view_changed,
            view_payloads: false,
            measured_deferred_light: false,
        };

        // `draw_frame` works on the renderer next to `self`'s other state;
        // none of the helpers it calls read `self.inner`.
        let mut inner = self.inner.take()?;
        changes.view_payloads = inner.apply_view_payloads(editor_state.view_payloads);
        let submission = self.draw_frame(
            &mut inner,
            &req,
            &clock,
            phases,
            changes,
            editor_state.ortho_half_height,
        );
        self.inner = Some(inner);
        submission
    }

    /// Advance the frame clock and counters; `dt` is zero for a capture tile.
    pub(super) fn tick_frame_clock(&mut self, start: Instant, capture: bool) -> FrameClock {
        let now = Instant::now();
        let dt = if capture {
            0.0
        } else {
            now.duration_since(self.last_frame).as_secs_f32().min(0.1)
        };
        self.last_frame = now;
        self.frame_count += 1;
        self.profiler_frame_counter += 1;
        FrameClock { start, now, dt }
    }

    /// Hand the renderer its frame budget and apply a queued gizmo mode.
    pub(super) fn apply_frame_start_gizmo_state(&mut self) {
        if let Some(inner) = &mut self.inner {
            // Every frame: a rebuilt scene renderer starts without it.
            inner.scene.renderer.set_frame_budget(self.frame_budget);
            if let Ok(mut pending) = self.pending_gizmo_mode.lock() {
                if let Some(mode) = pending.take() {
                    inner.interaction.set_mode(mode);
                    self.gizmo_dirty = true;
                }
            }
        }
    }

    /// Run the pointer events the UI thread queued (see
    /// `PendingPointerEvent`'s doc).
    ///
    /// Drained unconditionally, before the idle and pending-scene checks that
    /// follow: `handle_left_click`/`handle_left_release` already set
    /// `self.gizmo_dirty = true` internally, so processing them here keeps
    /// this frame from idling out on a drag-release commit.
    pub(super) fn process_pending_pointer_events(&mut self, capture: bool) {
        let mut pending_pointer_events = if capture {
            Vec::new()
        } else {
            profiling::profile_scope!("helio_take_pending_pointer_events");
            let mut batch = std::mem::take(&mut self.pointer_events_scratch);
            if let Ok(mut events) = self.pending_pointer_events.lock() {
                std::mem::swap(&mut batch, &mut *events);
            }
            batch
        };
        pointer_events_span!(!pending_pointer_events.is_empty());
        if !pending_pointer_events.is_empty() {
            profiling::profile_scope!("helio_pointer_events");
            for event in pending_pointer_events.drain(..) {
                self.apply_pointer_event(event);
            }
        }
        self.pointer_events_scratch = pending_pointer_events;
    }

    /// Run one queued pointer event.
    pub(super) fn apply_pointer_event(&mut self, event: PendingPointerEvent) {
        match event {
            PendingPointerEvent::MouseMove { norm_x, norm_y } => {
                profiling::profile_scope!("helio_handle_mouse_move");
                self.handle_mouse_move(norm_x, norm_y);
            }
            PendingPointerEvent::LeftClick { norm_x, norm_y } => {
                profiling::profile_scope!("helio_handle_left_click");
                self.handle_left_click(norm_x, norm_y);
            }
            PendingPointerEvent::LeftRelease => {
                profiling::profile_scope!("helio_handle_left_release");
                self.handle_left_release();
            }
            PendingPointerEvent::BoxSelect { from, to } => {
                profiling::profile_scope!("helio_handle_box_select");
                self.handle_box_select(from, to);
            }
            PendingPointerEvent::ToolRay {
                norm_x,
                norm_y,
                start,
                tool,
            } => {
                profiling::profile_scope!("helio_tool_ray");
                let (origin, direction) = self.pointer_ray(norm_x, norm_y);
                if direction != DVec3::ZERO {
                    (tool.0)(
                        &mut self.scene_store.write().world,
                        origin,
                        direction,
                        start,
                    );
                    self.gizmo_dirty = true;
                }
            }
        }
    }

    /// Note camera input activity before it is consumed; returns whether the
    /// viewport was asked to resize, or `None` when the input lock is poisoned.
    pub(super) fn poll_camera_input(&mut self, capture: bool) -> Option<bool> {
        let (had_input, needs_resize) = if capture {
            (false, false)
        } else {
            profiling::profile_scope!("helio_read_camera_input");
            let Ok(input) = self.camera_input.lock() else {
                return None;
            };
            (
                input.forward != 0.0
                    || input.right != 0.0
                    || input.up != 0.0
                    || input.mouse_delta_x != 0.0
                    || input.mouse_delta_y != 0.0
                    || input.pan_delta_x != 0.0
                    || input.pan_delta_y != 0.0
                    || input.zoom_delta != 0.0,
                input.needs_resize,
            )
        };
        if had_input {
            self.had_camera_input = true;
        }
        Some(needs_resize)
    }

    /// Apply the view kind, space traversal and camera input, then sync the
    /// camera to its peers.
    pub(super) fn update_camera(&mut self, dt: f32, capture: bool) -> CameraUpdate {
        let view_kind_changed = self.apply_view_kind();
        let follow = self.follow_space_transitions.load(Ordering::Acquire)
            && self
                .inner
                .as_ref()
                .is_some_and(|inner| inner.scene.camera_traversal_enabled());
        if self.active_space_traversal != follow {
            self.active_space_traversal = follow;
            self.reset_taa_next_frame = true;
        }
        if !capture {
            profiling::profile_scope!("helio_camera_input");
            self.apply_camera_input(dt);
        }
        let camera_synced = self.sync_camera();
        CameraUpdate {
            view_kind_changed,
            camera_synced,
        }
    }

    /// Apply the post-process baseline and debug view, and take the per-view
    /// payloads the UI queued. Runs before idle detection: a changed baseline
    /// renders this frame.
    pub(super) fn apply_editor_baselines(&mut self) -> EditorFrameState {
        let postprocess_changed = {
            profiling::profile_scope!("helio_apply_postprocess_baseline");
            self.apply_postprocess_baseline()
        };
        let debug_view_changed = self.apply_viewport_debug_mode();
        let ortho_half_height = self.ortho_half_height();
        let view_payloads = self
            .pending_view_payloads
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default();
        EditorFrameState {
            postprocess_changed,
            debug_view_changed,
            ortho_half_height,
            view_payloads,
        }
    }

    /// The part of a frame that works on the renderer: idle check, scene
    /// step, frame preparation, submission and profiler reporting.
    pub(super) fn draw_frame(
        &mut self,
        inner: &mut HelioInner,
        req: &FrameRequest<'_>,
        clock: &FrameClock,
        mut phases: FramePhases,
        mut changes: FrameChanges,
        ortho_half_height: f32,
    ) -> Option<wgpu::SubmissionIndex> {
        let SurfaceSpec { width, height, .. } = req.surface;
        let crop = req.crop;
        // Passes whose output shows their own cost (the lighting's cost view)
        // get their measured GPU time.
        changes.measured_deferred_light = inner.scene.renderer.feed_measured_gpu_times();

        let activity = self.detect_activity(inner, req, clock, &changes);
        if activity.camera_stopped {
            // Clear the sticky input flag when camera actually stopped.
            self.had_camera_input = false;
        }
        if activity.viewport_resized {
            self.resize_renderer(inner, req.surface, activity.scene_revision);
        }
        self.apply_pending_editor_commands(inner);

        // No GPU work, no gizmo rebuild, no profiler reads.
        if activity.is_idle {
            self.publish_idle_frame(clock.dt);
            return None;
        }

        let sync_ms = self.step_scene(inner, crop.is_none(), activity.has_pending_scene);
        phases.mark("scene");

        // SceneDB Inspector bridge: throttled inside SceneDB, and a no-op unless
        // an inspector launched this process. After the GPU flush above.
        {
            profiling::profile_scope!("helio_publish_inspector_snapshot");
            self.scene_store.read().world.publish_inspector_snapshot();
        }
        phases.mark("inspector");

        self.track_camera_ground(inner, &mut phases);
        let t_prepare = Instant::now();
        let frame = self.prepare_scene_frame(inner, req, ortho_half_height);
        let camera = frame.camera;
        if let Some(errors) = frame.new_errors {
            if let Ok(mut pending) = self.pending_errors.lock() {
                pending.extend(errors);
            }
        }
        phases.mark("publish");
        self.draw_editor_debug(inner);
        self.reset_taa_history_if_requested(inner, crop.is_some());

        let prepare_ms = t_prepare.elapsed().as_secs_f64() * 1000.0;
        self.log_voxel_stats(inner, width, height);
        phases.mark("prepare");

        let t_render = Instant::now();
        let (submission_index, render_succeeded) =
            self.submit_frame(inner, req, &camera, activity.realtime, &mut phases);
        self.gizmo_dirty = false;
        inner.has_rendered_frame = true;
        let render_ms = t_render.elapsed().as_secs_f64() * 1000.0;
        phases.mark("render");

        let frame_ms = self.record_frame_timing(clock, phases, render_ms);
        self.report_gpu_profile(
            inner,
            FrameTimes {
                frame_ms,
                sync_ms,
                prepare_ms,
                render_ms,
            },
        );

        {
            profiling::profile_scope!("helio_update_metrics");
            self.update_frame_metrics(clock.dt);
        }

        self.temporal_settling.complete_frame(
            inner
                .scene
                .renderer
                .find_pass::<helio_pass_tsr::TsrPass>()
                .is_some(),
            activity.temporal_activity,
            render_succeeded && submission_index.is_some(),
        );
        submission_index
    }

    /// Decide whether this frame has to draw.
    ///
    /// If the camera is fully stopped, no scene changes are pending, and no
    /// editor state changed, the GPU render is skipped entirely. The render
    /// thread keeps pacing itself but returns `None`, which causes the
    /// background loop to skip present/publish: the compositor holds the last
    /// frame on screen.
    pub(super) fn detect_activity(
        &mut self,
        inner: &HelioInner,
        req: &FrameRequest<'_>,
        clock: &FrameClock,
        changes: &FrameChanges,
    ) -> FrameActivity {
        let SurfaceSpec { width, height, .. } = req.surface;
        let crop = req.crop;
        let viewport_resized = changes.needs_resize || self.viewport_size != (width, height);
        let realtime = crop.is_none() && self.viewport_realtime.load(Ordering::Acquire);
        let (scene_revision, animated) = {
            profiling::profile_scope!("helio_scene_store_read (revision)");
            let store = self.scene_store.read();
            // Re-checked only when the revision changed.
            let animated = realtime && self.animated_content.live(&store.world);
            (store.world.revision(), animated)
        };
        // A newly-created/loaded SceneDB can have revision 0. The first
        // renderer frame still steps the database so its GPU mirror is current
        // before Helio reads it.

        let needs_initial_scene_sync = !inner.has_rendered_frame;
        // Every scene change -- edits, undo/redo, opening a level -- goes
        // through the World's normal write path and advances its revision,
        // so the revision alone wakes the renderer; there is no resync.
        let has_pending_scene =
            needs_initial_scene_sync || scene_revision != inner.last_scene_revision;
        let has_pending_editor = self.pending_deselect.load(Ordering::Acquire)
            || self.pending_gizmo_mode.lock().is_ok_and(|g| g.is_some());
        let camera_stopped = self.cam_local_velocity.length_squared() <= CAMERA_IDLE_EPSILON
            && !self.had_camera_input;
        // The settling budget itself is not activity: it must eventually drain.
        let temporal_activity = !camera_stopped
            || has_pending_scene
            || has_pending_editor
            // Content streaming in, building, or waiting on the renderer
            // (a sculpt stroke's picks: idling mid-drag stalled the stroke).
            || inner.scene.needs_frame()
            || self.gizmo_dirty
            || viewport_resized
            || changes.postprocess
            || changes.debug_view
            || changes.view_payloads
            || (self.viewport_debug_mode.load(Ordering::Acquire) == 34
                && changes.measured_deferred_light)
            || changes.view_kind
            || changes.camera_synced
            || crop.is_some()
            // Realtime animation: wind, particles, shader-graph `time`.
            || animated
            || self.reset_taa_next_frame;
        if self.activity_log
            && temporal_activity
            && clock
                .now
                .duration_since(self.last_activity_log)
                .as_secs_f64()
                >= 1.0
        {
            // Why the viewport keeps rendering (PULSAR_VOXEL_ACTIVITY=1).
            self.last_activity_log = clock.now;
            tracing::info!(
                camera_moving = !camera_stopped,
                scene = has_pending_scene,
                scene_revision,
                editor = has_pending_editor,
                content = inner.scene.needs_frame(),
                gizmo = self.gizmo_dirty,
                resized = viewport_resized,
                postprocess = changes.postprocess,
                animated,
                reset_taa = self.reset_taa_next_frame,
                "VOXEL_ACTIVITY"
            );
        }
        self.temporal_settling.observe_activity(
            inner
                .scene
                .renderer
                .find_pass::<helio_pass_tsr::TsrPass>()
                .is_some(),
            temporal_activity,
        );
        let is_idle = !temporal_activity && !self.temporal_settling.needs_frame();
        FrameActivity {
            viewport_resized,
            realtime,
            scene_revision,
            has_pending_scene,
            temporal_activity,
            is_idle,
            camera_stopped,
        }
    }
}
