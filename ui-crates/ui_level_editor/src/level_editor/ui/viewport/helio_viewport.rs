//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! Follows the reference example `wgpu_surface_basic.rs`:
//!   1. Create `WgpuSurfaceHandle` lazily on first render.
//!   2. Each frame: `back_view_with_size()` → render → `swap_buffers()`.
//!   3. Return `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use plugin_editor_api::{AssetKind, AssetPayload};
use ui::{notification::Notification, ActiveTheme as _, ContextModal};

/// A GPUI component that drives the Helio renderer into a `WgpuSurfaceHandle`.
pub struct HelioViewport {
    pub gpu_engine: Arc<Mutex<GpuRenderer>>,
    surface: Option<WgpuSurfaceHandle>,
    focus_handle: FocusHandle,
    debug_replace_with_yellow: bool,
}

impl HelioViewport {
    pub fn new<V: 'static>(
        gpu_engine: Arc<Mutex<GpuRenderer>>,
        debug_replace_with_yellow: bool,
        cx: &mut Context<V>,
    ) -> Self {
        Self {
            gpu_engine,
            surface: None,
            focus_handle: cx.focus_handle(),
            debug_replace_with_yellow,
        }
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

        // Show importing notification
        window.push_notification(
            Notification::info("Importing Asset").message(format!("Loading {}...", name)),
            cx,
        );

        let result = Self::import_asset(path, kind, self.gpu_engine.clone());
        match result {
            Ok(()) => {
                window.push_notification(
                    Notification::success("Import Successful")
                        .message(format!("Imported {}", name)),
                    cx,
                );
            }
            Err(e) => {
                tracing::error!("Failed to import {}: {}", name, e);
                window.push_notification(
                    Notification::error("Import Failed")
                        .message(format!("Failed to import {}: {}", name, e)),
                    cx,
                );
            }
        }
    }

    /// Load and insert the dropped asset into the scene.
    fn import_asset(
        path: PathBuf,
        kind: AssetKind,
        gpu_engine: Arc<Mutex<GpuRenderer>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Validate file exists
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()).into());
        }

        // Based on AssetKind, load different asset types
        match kind {
            AssetKind::Mesh | AssetKind::Scene => {
                tracing::info!("Loading FBX/scene from {:?}", path);

                // Load the scene file using helio-asset-compat
                let load_config = helio_asset_compat::LoadConfig {
                    flip_uv_y: true,            // Convert DirectX → OpenGL UVs
                    merge_meshes: false,        // Keep separate meshes
                    import_scale: glam::Vec3::ONE, // 1:1 scale
                };

                let converted_scene = helio_asset_compat::load_scene_file_with_config(&path, load_config)
                    .map_err(|e| format!("Failed to load scene file {}: {}", path.display(), e))?;

                tracing::info!(
                    "Loaded scene: {} meshes, {} materials, {} textures",
                    converted_scene.meshes.len(),
                    converted_scene.materials.len(),
                    converted_scene.textures.len()
                );

                // Insert the scene into Helio. We use a blocking lock here so
                // drops don't fail transiently during a render tick.
                let mut engine = gpu_engine
                    .lock()
                    .map_err(|_| "Failed to lock GPU renderer")?;
                engine
                    .insert_scene_object(converted_scene)
                    .map_err(|e| format!("Failed to insert scene object: {}", e))?;
                tracing::info!("Scene successfully inserted into Helio renderer");
            }
            _ => {
                return Err(format!("Unsupported asset type: {:?}", kind).into());
            }
        }

        Ok(())
    }
}

impl Focusable for HelioViewport {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for HelioViewport {}

impl Render for HelioViewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.debug_replace_with_yellow {
            return div()
                .relative()
                .size_full()
                .track_focus(&self.focus_handle)
                .id("helio_viewport_debug_yellow")
                .bg(rgb(0xffff00))
                .into_any_element();
        }

        // Keep rendering continuously for real-time 3D viewport updates.
        window.request_animation_frame();

        let format = wgpu::TextureFormat::Rgba8UnormSrgb;

        // Lazy surface creation (once).
        if self.surface.is_none() {
            match window.create_wgpu_surface(1600, 900, format) {
                Some(s) => {
                    tracing::info!("[HELIO-VIEWPORT] WgpuSurface created (format={:?})", format);
                    self.surface = Some(s);
                }
                None => {
                    tracing::warn!("[HELIO-VIEWPORT] create_wgpu_surface returned None");
                }
            }
        }

        // Render into the back buffer, then swap.  If the surface is still
        // resizing, keep the previous display buffer visible and avoid forcing
        // Helio to resize mid-drag.
        if let Some(ref surface) = self.surface {
            if !surface.is_resize_pending() {
                if let Some((view, (w, h))) = surface.back_view_with_size() {
                    if let Ok(mut engine) = self.gpu_engine.try_lock() {
                        engine.render_frame_to_surface(
                            surface.device(),
                            surface.queue(),
                            &view,
                            w,
                            h,
                            surface.format(),
                        );
                    }
                    drop(view);
                    surface.swap_buffers();
                }
            }
        }

        // Build the viewport element
        let viewport_element = if let Some(ref surface) = self.surface {
            wgpu_surface(surface.clone())
                .defer_resize_until_mouse_up(true)
                .absolute()
                .inset_0()
                .into_any_element()
        } else {
            div()
                .relative()
                .track_focus(&self.focus_handle)
                .id("helio_viewport")
                .size_full()
                .bg(rgb(0xff0000))
                .child("Waiting for WgpuSurface...")
                .into_any_element()
        };

        // Accept mesh/scene payload drags and forward successful drops to the viewport entity.
        let viewport = cx.entity().clone();
        div()
            .id("helio-viewport-drop")
            .size_full()
            .drag_over::<AssetPayload>(|style, payload, _window, cx| {
                if matches!(payload.kind, AssetKind::Mesh | AssetKind::Scene) {
                    style
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded(px(4.0))
                } else {
                    style.opacity(0.4)
                }
            })
            .on_drop::<AssetPayload>(move |payload, window, cx| {
                if matches!(payload.kind, AssetKind::Mesh | AssetKind::Scene) {
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
