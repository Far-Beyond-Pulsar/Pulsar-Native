//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! Follows the reference example `wgpu_surface_basic.rs`:
//!   1. Create `WgpuSurfaceHandle` lazily on first render.
//!   2. Each frame: `back_view_with_size()` → render → `swap_buffers()`.
//!   3. Return `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use plugin_editor_api::{AssetKind, AssetPayload};
use ui::{ActiveTheme as _, ContextModal, notification::Notification};

use crate::level_editor::commands::{SceneCommand, execute_command};
use crate::level_editor::scene_database::{MeshType, ObjectType, SceneObjectData, Transform};
use crate::level_editor::state::LevelEditorState;
use pulsar_rendering::asset_component::component_class_for_asset;
use pulsar_reflection::REGISTRY;

/// A GPUI component that drives the Helio renderer into a `WgpuSurfaceHandle`.
///
/// Rendering runs on a dedicated background thread so the UI thread is never
/// blocked by GPU work. The background thread owns a clone of the `GpuRenderer`
/// and the `WgpuSurfaceHandle`; it loops at ~60 FPS calling
/// `render_frame_to_surface()` + `present_synced_silent()`.
///
/// Presentation is deliberately split across the two threads:
/// - the render thread only publishes finished frames into the triple buffer;
/// - the UI thread keeps the GPUI draw cycle alive via `request_animation_frame()`,
///   and the compositor promotes `ready` -> `display` only when a new frame exists.
pub struct HelioViewport {
    pub gpu_engine: Arc<Mutex<GpuRenderer>>,
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    surface: Option<WgpuSurfaceHandle>,
    focus_handle: FocusHandle,
    debug_replace_with_yellow: bool,
    tab_activated: Arc<AtomicBool>,
    render_thread_stop: Arc<AtomicBool>,
    render_thread_handle: Option<std::thread::JoinHandle<()>>,
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
            Notification::info("Adding to Scene").message(format!("Placing {}...", name)),
            cx,
        );

        let result = Self::import_asset(path, kind, self.shared_state.clone());
        match result {
            Ok(()) => {
                window.push_notification(
                    Notification::success("Added to Scene").message(format!("Placed {}", name)),
                    cx,
                );
            }
            Err(e) => {
                tracing::error!("Failed to place {}: {}", name, e);
                window.push_notification(
                    Notification::error("Placement Failed")
                        .message(format!("Failed to place {}: {}", name, e)),
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
    fn start_render_thread(&mut self, surface: WgpuSurfaceHandle) {
        if self.render_thread_handle.is_some() {
            return;
        }

        let engine = self.gpu_engine.clone();
        let stop = self.render_thread_stop.clone();
        let tab_activated = self.tab_activated.clone();

        let handle = std::thread::Builder::new()
            .name("Helio Render".into())
            .spawn(move || {
                profiling::set_thread_name("Helio Render");
                let mut first_frame = true;

                loop {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }

                    if !first_frame {
                        std::thread::sleep(Duration::from_millis(16));
                    }
                    first_frame = false;

                    // Permission to submit on the shared device. Held across
                    // render + present so a window resize (which reconfigures the
                    // swapchain and requires an idle queue) cannot race our submit.
                    // This is a read guard: other surfaces' render threads hold
                    // theirs concurrently, so frame pacing stays independent.
                    let submit_guard = surface.submit_guard();

                    let Some((view, (width, height))) = surface.back_view_with_size() else {
                        continue;
                    };

                    let device = surface.device();
                    let queue = surface.queue();
                    let format = surface.format();

                    let submission_index = match engine.lock() {
                        Ok(mut engine) => {
                            if tab_activated.swap(false, Ordering::AcqRel) {
                                engine.reset_taa();
                            }
                            engine.render_frame_to_surface(
                                device, queue, &view, width, height, format,
                            )
                        }
                        Err(_) => None,
                    };

                    drop(view);

                    if let Some(idx) = submission_index {
                        // Silent present: publish the frame for the compositor but do
                        // NOT request a window redraw from this thread. The GPUI draw
                        // cycle is kept alive by `request_animation_frame()` in
                        // `render()`; driving repaints from here instead would fire a
                        // winit `RedrawRequested` per frame, and each one that misses
                        // the fast-blit path forces a full `window.refresh()` of the
                        // entire editor UI.
                        surface.present_synced_silent(idx);
                    }

                    drop(submit_guard);
                }
            });

        match handle {
            Ok(h) => self.render_thread_handle = Some(h),
            Err(e) => tracing::error!("Failed to spawn Helio render thread: {:?}", e),
        }
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
            match window.create_wgpu_surface(1600, 900, format) {
                Some(s) => {
                    tracing::info!("[HELIO-VIEWPORT] WgpuSurface created (format={:?})", format);
                    self.start_render_thread(s.clone());
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
                    Notification::error("Mesh Load Failed").message(err),
                    cx,
                );
            }
        }

        // Capture a project thumbnail if a save just requested one.
        // This must happen synchronously since it reads back GPU data.
        let capture_path = self.shared_state.write().build.pending_thumbnail_capture.take();
        if let Some(path) = capture_path {
            if let Some(ref surface) = self.surface {
                if let Some((view, (w, h))) = surface.back_view_with_size() {
                    if let Ok(mut engine) = self.gpu_engine.try_lock() {
                        capture_viewport_thumbnail(
                            &mut engine, surface, w, h, format, &path,
                        );
                    }
                }
            }
        }

        // Build the viewport element
        let viewport_element = if let Some(ref surface) = self.surface {
            // Keep the GPUI draw cycle running so the compositor keeps promoting
            // frames produced by the render thread, and so `WgpuSurface::prepaint`
            // keeps observing the element's bounds (which is what resizes the
            // surface when the viewport panel changes size).
            window.request_animation_frame();

            wgpu_surface(surface.clone())
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
