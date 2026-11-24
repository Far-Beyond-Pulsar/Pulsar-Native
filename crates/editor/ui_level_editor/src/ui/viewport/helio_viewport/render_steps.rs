//! The steps of `HelioViewport::render`: surface lifetime, pending captures
//! and the element tree.

use super::*;

impl HelioViewport {
    /// The GPU device was lost (a driver reset, a removed GPU): stop drawing
    /// on it. The window rebuilds itself on a new device and renders every
    /// view again, and the surface made then is on that one; the renderer
    /// follows the new device (`HelioRenderer` rebuilds on it).
    pub(super) fn drop_lost_surface(&mut self) {
        if self
            .surface
            .as_ref()
            .is_some_and(WgpuSurfaceHandle::is_device_lost)
        {
            tracing::warn!("[HELIO-VIEWPORT] GPU device lost; waiting for the window's new device");
            // The runtime marks itself lost from its output and builds again
            // on the next one.
            self.surface = None;
        }
    }

    /// Create the surface the runtime draws into, when there is none yet.
    pub(super) fn ensure_surface(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        format: wgpu::TextureFormat,
    ) {
        if self.surface.is_some() {
            return;
        }
        // If this comes up empty (Wayland before `wl_surface.enter`), the
        // frame pump keeps polling via `poll_display_refresh`.
        self.last_display_refresh_sample = Instant::now();
        let refresh_millihertz = Self::sample_display_refresh_millihertz(window, cx);
        if let Some(millihertz) = refresh_millihertz {
            self.display_refresh_millihertz
                .store(millihertz, Ordering::Relaxed);
        }
        let refresh_hz = refresh_millihertz.map(|millihertz| millihertz as f64 / 1000.0);

        match window.create_wgpu_surface_with_color_conversion(
            1600,
            900,
            format,
            gpui::SurfaceColorConversion::LinearToSrgb,
        ) {
            // Until the window has rebuilt on a new device, a new surface
            // is on the lost one too; the rebuild renders this view again.
            Some(s) if s.is_device_lost() => {}
            Some(s) => {
                tracing::info!("[HELIO-VIEWPORT] WgpuSurface created (format={:?})", format);
                let _ = refresh_hz;
                self.begin_initial_load();
                // Let WGPUI present this frame before the runtime builds
                // its cold pipeline graph on the shared device: the output
                // is handed over at the start of the next platform frame.
                let runtime = self.runtime.clone();
                let target = Arc::new(ui_helio_viewport::SurfaceTarget::new(
                    s.clone(),
                    self.frames_published.clone(),
                    self.display_refresh_millihertz.clone(),
                ));
                window.on_next_frame(move |_, _| runtime.set_output(target));
                self.surface = Some(s);
            }
            None => {
                tracing::warn!("[HELIO-VIEWPORT] create_wgpu_surface returned None");
            }
        }
    }

    /// Show the loading overlay while the runtime builds its first renderer.
    fn begin_initial_load(&mut self) {
        if self.initial_load.is_none() && self.runtime.frames() == 0 {
            let loading = self
                .shared_state
                .read()
                .editor_loading
                .begin("Initializing Helio renderer…");
            loading.set_progress(0.05);
            self.initial_load = Some(loading);
        }
    }

    /// Drain pending mesh-load errors (non-blocking) into notifications.
    pub(super) fn drain_mesh_load_errors(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(engine) = self.gpu_engine.try_lock() {
            for err in engine.drain_pending_errors() {
                window.push_notification(
                    Notification::error(t!("Notification.Title.MeshLoadFailed").to_string())
                        .message(err),
                    cx,
                );
            }
        }
    }

    /// Capture a project thumbnail if a save just requested one.
    /// This must happen synchronously since it reads back GPU data.
    pub(super) fn capture_pending_thumbnail(&mut self, format: wgpu::TextureFormat) {
        let capture_path = self
            .shared_state
            .write()
            .build
            .pending_thumbnail_capture
            .take();
        if let Some(path) = capture_path {
            if let Some(ref surface) = self.surface {
                if let Some((_view, (w, h))) = surface.back_view_with_size() {
                    if let Ok(mut engine) = self.gpu_engine.try_lock() {
                        capture_viewport_thumbnail(&mut engine, surface, w, h, format, &path);
                    }
                }
            }
        }
    }

    /// Start a requested high-resolution screenshot.
    ///
    /// Keeps exclusive access to Helio across every tile so the projection,
    /// scene state and frozen scene clock stay consistent until assembly ends.
    pub(super) fn start_pending_screenshot(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        format: wgpu::TextureFormat,
    ) {
        let screenshot_size = self
            .shared_state
            .write()
            .build
            .pending_screenshot_size
            .take();
        let Some((width, height)) = screenshot_size else {
            return;
        };
        let Some(surface) = self.surface.clone() else {
            let mut state = self.shared_state.write();
            state.build.screenshot_processing = false;
            state.build.screenshot_progress = None;
            return;
        };
        let temp_path = std::env::temp_dir().join(format!(
            "pulsar-screenshot-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let capture =
            self.spawn_screenshot_capture(cx, surface, (width, height), format, temp_path.clone());
        self.save_screenshot_when_ready(window, cx, capture, temp_path);
    }

    /// Render the screenshot tiles on a background thread into `output_path`.
    fn spawn_screenshot_capture(
        &self,
        cx: &mut Context<Self>,
        surface: WgpuSurfaceHandle,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
        output_path: PathBuf,
    ) -> Task<Result<PathBuf, String>> {
        let engine = self.gpu_engine.clone();
        let progress_state = self.shared_state.clone();
        cx.background_executor().spawn(async move {
            let mut engine = engine
                .lock()
                .map_err(|_| "Helio renderer lock was poisoned".to_string())?;
            // Keep renderer panics inside this lock guard so they are
            // reported as capture failures instead of poisoning the
            // viewport renderer mutex or unwinding through the task.
            catch_unwind(AssertUnwindSafe(|| {
                capture_viewport_png_tiled(
                    &mut engine,
                    &surface,
                    width,
                    height,
                    format,
                    &output_path,
                    move |completed, total| {
                        progress_state.write().build.screenshot_progress = Some((completed, total));
                    },
                )
            }))
            .map_err(|payload| {
                let message = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&'static str>().copied())
                    .unwrap_or("unknown panic");
                format!("Helio panicked while rendering a screenshot tile: {message}")
            })??;
            Ok::<_, String>(output_path)
        })
    }

    /// When the capture finishes, report a failure or ask where to save the
    /// screenshot, then remove the temporary file.
    fn save_screenshot_when_ready(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        capture: Task<Result<PathBuf, String>>,
        temp_path: PathBuf,
    ) {
        let state = self.shared_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = capture.await;
            {
                let mut state = state.write();
                state.build.screenshot_processing = false;
                state.build.screenshot_progress = None;
            }
            let temp_path = match result {
                Ok(path) => path,
                Err(error) => {
                    tracing::warn!("High-resolution screenshot capture failed: {error}");
                    remove_temp_screenshot(&temp_path);
                    this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error("Screenshot capture failed").message(error),
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
            };
            let dialog = rfd::AsyncFileDialog::new()
                .set_title("Save High Res Screenshot")
                .add_filter("PNG image", &["png"])
                .set_file_name("screenshot.png");
            if let Some(file) = dialog.save_file().await {
                let path = file.path().to_path_buf();
                if let Err(error) = copy_screenshot(&temp_path, &path) {
                    tracing::warn!("Failed to save high-resolution screenshot: {error}");
                    this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error("Could not save screenshot")
                                .message(error.to_string()),
                            cx,
                        );
                    })
                    .ok();
                }
            }
            remove_temp_screenshot(&temp_path);
        })
        .detach();
    }

    /// The element showing the surface (and starting the frame pump), or an
    /// empty placeholder before the surface exists.
    pub(super) fn viewport_element(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(surface) = self.surface.clone() else {
            // Surface not created yet: render nothing rather than a placeholder.
            // This lasts a single frame, so any overlay just reads as a flash.
            return div()
                .relative()
                .track_focus(&self.focus_handle)
                .id("helio_viewport")
                .size_full()
                .into_any_element();
        };
        let (width, height) = surface.size();
        {
            let mut state = self.shared_state.write();
            state.build.viewport_size = (width, height);
            state.build.max_texture_dimension_2d =
                surface.device().limits().max_texture_dimension_2d;
        }
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
        self.last_full_render = Instant::now();
        self.viewport_size_at_last_full_render = Some(window.viewport_size());
        self.last_published_frame = self.frames_published.load(Ordering::Acquire);

        wgpu_surface(surface)
            .defer_resize_until_mouse_up(true)
            .absolute()
            .inset_0()
            .into_any_element()
    }

    /// Wrap the viewport in a drop target that accepts mesh, scene and
    /// blueprint payload drags and forwards successful drops to the viewport
    /// entity, with the screenshot progress overlay on top while one renders.
    pub(super) fn asset_drop_target(
        viewport_element: AnyElement,
        screenshot_processing: bool,
        screenshot_progress: Option<(u64, u64)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = cx.entity().clone();
        div()
            .id("helio-viewport-drop")
            .size_full()
            .on_drag_move::<AssetPayload>({
                let viewport = cx.entity().clone();
                move |event, window, cx| {
                    let payload = event.drag(cx).clone();
                    let local_x: f32 = (event.event.position.x - event.bounds.origin.x).into();
                    let local_y: f32 = (event.event.position.y - event.bounds.origin.y).into();
                    let width: f32 = event.bounds.size.width.into();
                    let height: f32 = event.bounds.size.height.into();
                    if width <= 0.0 || height <= 0.0 {
                        return;
                    }
                    let norm_x = local_x / width;
                    let norm_y = local_y / height;
                    if !(0.0..=1.0).contains(&norm_x) || !(0.0..=1.0).contains(&norm_y) {
                        return;
                    }
                    viewport.update(cx, |this, cx| {
                        this.placement_bounds = Some(event.bounds);
                        this.update_active_placement(&payload, norm_x, norm_y, window, cx)
                    });
                }
            })
            .drag_over::<AssetPayload>(|style, payload, _window, cx| {
                // Source models (FBX, …) are imported, not placed.
                if HelioViewport::supports_asset_placement(payload) {
                    style
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded(px(4.0))
                } else {
                    style.opacity(0.4)
                }
            })
            .on_drop::<AssetPayload>(move |payload, window, cx| {
                if payload.is_source_model() {
                    let payload = payload.clone();
                    viewport.update(cx, |this, cx| {
                        this.reject_source_model_drop(&payload, window, cx);
                    });
                } else if HelioViewport::supports_asset_placement(payload) {
                    let payload = payload.clone();
                    viewport.update(cx, |this, cx| {
                        this.handle_asset_drop(&payload, window, cx);
                    });
                }
            })
            .child(viewport_element)
            .when(screenshot_processing, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000099))
                        .text_color(rgb(0xffffff))
                        .child(match screenshot_progress {
                            Some((completed, total)) => format!(
                                "High resolution screenshot: {completed}/{total} tiles complete…"
                            ),
                            None => "High resolution screenshot is processing…".to_string(),
                        }),
                )
            })
            .into_any_element()
    }
}
