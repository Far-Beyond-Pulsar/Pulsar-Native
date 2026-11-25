//! Preparing the renderer for a frame: rebuilds on changed settings, pipeline
//! swaps, device changes and first-frame initialization.

use super::*;

impl HelioRenderer {
    /// Prepare or rebuild Helio's graph without holding WGPUI's submit guard.
    ///
    /// Graph construction can compile many pipelines. The editor calls this
    /// before taking the surface submit guard so a resize/reconfigure of the
    /// editor window is never held up by first-frame initialization.
    pub fn prepare_for_render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> bool {
        self.adopt_pending_camera_state();
        let graph_settings = self.graph_settings_cache.load();
        self.rebuild_on_graph_settings_change(&graph_settings);
        self.swap_pipeline_if_changed();
        if !self.device_is_usable(device) {
            return false;
        }
        if self.inner.is_none() {
            let surface = SurfaceSpec {
                width,
                height,
                format,
            };
            self.initialize_renderer(device, queue, surface, graph_settings);
        }
        true
    }

    /// Apply a camera pose queued by level loading.
    ///
    /// Level loading runs on the UI thread while rendering can hold the
    /// engine mutex. Consuming the mailbox before camera input and idle
    /// detection means the saved pose cannot be silently skipped.
    pub(super) fn adopt_pending_camera_state(&mut self) {
        let pending_camera = self
            .pending_camera_state
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        if let Some(camera) = pending_camera {
            self.set_editor_camera_state(camera);
            self.reset_taa_next_frame = true;
        }
    }

    /// Drop the renderer when graph-affecting project settings changed.
    ///
    /// These rebuild at a frame boundary, which keeps pass topology and its
    /// GPU allocations in sync with the UI.
    pub(super) fn rebuild_on_graph_settings_change(
        &mut self,
        graph_settings: &Arc<ProjectGraphSettings>,
    ) {
        if self.inner.is_some()
            && self.applied_graph_settings.as_ref().is_some_and(|applied| {
                !Arc::ptr_eq(applied, graph_settings) && !applied.same_graph(graph_settings)
            })
        {
            tracing::info!(
                "Rebuilding Helio renderer because graph-affecting project settings changed"
            );
            self.inner = None;
            self.applied_graph_settings = None;
            self.reset_taa_next_frame = true;
        }
    }

    /// Swap the pass topology when the level's pipeline changed.
    ///
    /// A different level pipeline, or the same asset saved again, is a
    /// different pass topology: the renderer swaps its pipeline at this frame
    /// boundary (the scene, its GPU mirror and the device stay). Compared as
    /// states, so a pipeline registering after it was requested (a project
    /// graph loading) replaces the status screen of "not found".
    pub(super) fn swap_pipeline_if_changed(&mut self) {
        let wanted_pipeline =
            super::super::pipelines::pipeline_state(self.requested_pipeline.get().as_deref());
        if self.applied_pipeline.as_ref() != Some(&wanted_pipeline) {
            if let Some(inner) = self.inner.as_mut() {
                tracing::info!(
                    previous_pipeline = ?self.applied_pipeline,
                    wanted_pipeline = ?wanted_pipeline,
                    "Swapping the Helio renderer's pipeline"
                );
                inner.scene.set_pipeline_state(wanted_pipeline.clone());
                self.applied_pipeline = Some(wanted_pipeline);
                self.reset_taa_next_frame = true;
                self.publish_debug_views();
            }
        }
    }

    /// Whether frames can be drawn on `device`; false once it is lost.
    ///
    /// A frame for another device (the old one was lost and a new one
    /// made) rebuilds the renderer there; `ensure_gpu_mirror` moves the
    /// scene's GPU copy along. On a lost device nothing is drawn: every
    /// call would fail.
    pub(super) fn device_is_usable(&mut self, device: &wgpu::Device) -> bool {
        if self
            .inner
            .as_ref()
            .is_some_and(|inner| inner.device != *device)
        {
            tracing::warn!("Viewport device changed; rebuilding the Helio renderer on it");
            self.inner = None;
            self.applied_graph_settings = None;
            self.reset_taa_next_frame = true;
            self.reported_device_loss = false;
        } else if self
            .device_loss
            .as_ref()
            .is_some_and(crate::services::device_loss::DeviceLoss::is_lost)
        {
            if !self.reported_device_loss {
                self.reported_device_loss = true;
                tracing::error!(
                    "GPU device lost; the viewport stops drawing until it has a new one"
                );
            }
            return false;
        }
        true
    }

    /// Build the scene renderer, graph and pipelines on the first frame.
    ///
    /// They are built inline on the render thread, so the task system tracks
    /// and logs them: a hang here is visible in the Tasks window with this
    /// phase, instead of a silently blank viewport.
    pub(super) fn initialize_renderer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: SurfaceSpec,
        graph_settings: Arc<ProjectGraphSettings>,
    ) {
        editor_task_queue::scope(
            editor_task_queue::TaskDescription::foreground("Initialize Helio renderer", "render"),
            |task| {
                task.set_phase("Creating the scene renderer and render graph");
                frame_span!("pulsar: HelioRenderer::lazy_init");
                tracing::info!("Initializing Helio renderer...");
                self.create_inner(device, queue, surface, graph_settings);
                task.set_phase("Renderer ready");
            },
        );
    }

    /// Create `HelioInner` on `device` and record what it was built from.
    pub(super) fn create_inner(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: SurfaceSpec,
        graph_settings: Arc<ProjectGraphSettings>,
    ) {
        let SurfaceSpec {
            width,
            height,
            format,
        } = surface;
        self.device_loss = Some(crate::services::device_loss::DeviceLoss::watch(device));
        // The scene's renderer as the game builds it; the editor adds its
        // background, overlays and debug views.
        let mut scene = SceneRenderer::new(
            Arc::new(device.clone()),
            Arc::new(queue.clone()),
            &self.scene_store,
            width,
            height,
            format,
            SceneRendererOptions {
                editor_mode: true,
                device: DeviceOwner::Host,
                pipeline: self.requested_pipeline.get(),
                construct_pump: self.construct_pump.clone(),
                host_part_setups: self.host_part_setups.clone(),
                pipeline_build: self.pipeline_build,
            },
        );
        scene.renderer.set_clear_color([0.15, 0.18, 0.25, 1.0]);
        self.applied_pipeline = Some(scene.pipeline().clone());

        let inner = HelioInner {
            scene,
            device: device.clone(),
            queue: Arc::new(queue.clone()),
            interaction: SceneInteraction::default(),
            last_scene_revision: 0,
            has_rendered_frame: false,
            applied_postprocess: None,
            applied_debug_mode: 0,
            editor_lines: None,
        };
        self.inner = Some(inner);
        self.publish_debug_views();
        self.applied_graph_settings = Some(graph_settings);
        self.viewport_size = (width, height);

        let (yaw, pitch) = self.cam.local_yaw_pitch();
        tracing::info!(
            width,
            height,
            "[HELIO] Renderer initialized - camera at {:?}, yaw={yaw}, pitch={pitch}",
            self.cam.position,
        );
    }
}
