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

use crate::level_editor::commands::{execute_command, SceneCommand};
use crate::level_editor::scene_database::{MeshType, ObjectType, SceneObjectData, Transform};
use crate::level_editor::ui::state::LevelEditorState;

/// A GPUI component that drives the Helio renderer into a `WgpuSurfaceHandle`.
pub struct HelioViewport {
    pub gpu_engine: Arc<Mutex<GpuRenderer>>,
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    surface: Option<WgpuSurfaceHandle>,
    focus_handle: FocusHandle,
    debug_replace_with_yellow: bool,
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

                // Build a __component_instances entry for StaticMeshComponent so the
                // renderer's sync_scene() loop loads the FBX via the component path.
                let mut props = std::collections::HashMap::new();
                props.insert(
                    "__component_instances".to_string(),
                    serde_json::json!([{
                        "class_name": "StaticMeshComponent",
                        "enabled": true,
                        "data": { "mesh_asset": asset_path }
                    }]),
                );

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
                    props,
                    scene_path: path.display().to_string(),
                };

                let add_result = execute_command(
                    &mut state,
                    SceneCommand::AddObject {
                        data: mesh_object,
                        parent_id: None,
                    },
                );

                if let Some(id) = add_result.affected_ids.first() {
                    let _ = execute_command(
                        &mut state,
                        SceneCommand::SelectObject {
                            id: Some(id.clone()),
                        },
                    );
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

                let mut props = std::collections::HashMap::new();
                props.insert(
                    "__component_instances".to_string(),
                    serde_json::json!([{
                        "class_name": "ScriptComponent",
                        "enabled": true,
                        "data": { "script_asset": script_path }
                    }]),
                );

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
                    props,
                    scene_path: path.display().to_string(),
                };

                let add_result = execute_command(
                    &mut state,
                    SceneCommand::AddObject {
                        data: blueprint_object,
                        parent_id: None,
                    },
                );

                if let Some(id) = add_result.affected_ids.first() {
                    let _ = execute_command(
                        &mut state,
                        SceneCommand::SelectObject {
                            id: Some(id.clone()),
                        },
                    );
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
                        let t_frame = std::time::Instant::now();
                        engine.render_frame_to_surface(
                            surface.device(),
                            surface.queue(),
                            &view,
                            w,
                            h,
                            surface.format(),
                        );
                        let frame_ms = t_frame.elapsed().as_secs_f64() * 1000.0;
                        if frame_ms > 16.0 {
                            tracing::warn!(
                                "[VIEWPORT] render_frame_to_surface took {:.1}ms",
                                frame_ms
                            );
                        }
                        for err in engine.drain_pending_errors() {
                            window.push_notification(
                                Notification::error("Mesh Load Failed").message(err),
                                cx,
                            );
                        }
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
