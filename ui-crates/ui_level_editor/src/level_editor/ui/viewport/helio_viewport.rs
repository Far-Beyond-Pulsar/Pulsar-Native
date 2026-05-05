//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! Follows the reference example `wgpu_surface_basic.rs`:
//!   1. Create `WgpuSurfaceHandle` lazily on first render.
//!   2. Each frame: `back_view_with_size()` → render → `swap_buffers()`.
//!   3. Return `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::sync::{Arc, Mutex};

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use plugin_editor_api::{AssetDropArea, AssetDropAreaExt, AssetKind, AssetPayload};

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
        use std::path::PathBuf;
        use ui::notification::Notification;

        let path = PathBuf::from(&payload.engine_path);
        let name = payload.name.clone();
        let kind = payload.kind.clone();

        tracing::info!("Asset dropped on viewport: {} ({:?})", name, kind);

        // Show importing notification
        window.push_notification(
            Notification::info("Importing Asset").message(format!("Loading {}...", name)),
            cx,
        );

        // Spawn async import task
        let gpu_engine = self.gpu_engine.clone();
        cx.spawn(|_viewport, mut cx| async move {
            let result = Self::import_asset_async(path, kind, gpu_engine).await;

            // Update UI on main thread
            cx.update(|cx| {
                match result {
                    Ok(()) => {
                        cx.push_notification(
                            Notification::success("Import Successful")
                                .message(format!("Imported {}", name)),
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to import {}: {}", name, e);
                        cx.push_notification(
                            Notification::error("Import Failed")
                                .message(format!("Failed to import {}: {}", name, e)),
                        );
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Async asset import - loads and inserts the asset into the scene
    async fn import_asset_async(
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

                let converted_scene = tokio::task::spawn_blocking(move || {
                    helio_asset_compat::load_scene_file_with_config(&path, load_config)
                })
                .await??;

                tracing::info!(
                    "Loaded scene: {} meshes, {} materials, {} textures",
                    converted_scene.meshes.len(),
                    converted_scene.materials.len(),
                    converted_scene.textures.len()
                );

                // Insert the scene into Helio
                if let Ok(mut engine) = gpu_engine.try_lock() {
                    engine.insert_scene_object(converted_scene)?;
                    tracing::info!("Scene successfully inserted into Helio renderer");
                } else {
                    return Err("Failed to lock GPU renderer".into());
                }
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

        // Wrap in AssetDropArea to accept mesh and scene drops
        AssetDropArea::new("helio-viewport-drop")
            .accepts(vec![AssetKind::Mesh, AssetKind::Scene])
            .on_asset_drop(cx.listener(|this, payload, window, cx| {
                this.handle_asset_drop(payload, window, cx);
            }))
            .size_full()
            .child(viewport_element)
            .into_any_element()
    }
}
