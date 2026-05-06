use gpui::*;
use rust_i18n::t;
use ui::{
    dock::{DockItem, Panel, PanelEvent},
    resizable::ResizableState,
    v_flex,
    workspace::Workspace,
};
// HelioViewport — GPUI-native Helio 3D viewport
use super::viewport::helio_viewport::HelioViewport;

use engine_backend::services::gpu_renderer::{GpuRenderer, GpuRendererBuilder};
use engine_backend::GameThread;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use ui::settings::EngineSettings;
use ui_common::StatusBar;

use super::actions::*;
use super::{toolbar, CameraMode, LevelEditorState, ToolbarPanel, TransformTool, ViewportPanel};
use crate::level_editor::scene_database::{
    LightType, MeshType, ObjectType, SceneObjectData, Transform,
};
use engine_backend::scene::SceneDb;

/// Main Level Editor Panel - Orchestrates all sub-components
pub struct LevelEditorPanel {
    focus_handle: FocusHandle,

    // FPS graph type state (shared with viewport for Switch)
    fps_graph_is_line: Rc<RefCell<bool>>,

    // UI Components
    toolbar: ToolbarPanel,

    // Helio viewport rendered via WgpuSurfaceHandle
    viewport: Entity<HelioViewport>,
    gpu_engine: Arc<Mutex<GpuRenderer>>, // Full GPU renderer from backend
    render_enabled: Arc<std::sync::atomic::AtomicBool>,

    // Game thread for object movement and game logic
    game_thread: Arc<GameThread>,

    // Shared state for all panels (single source of truth)
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,

    // Workspace for draggable panels
    workspace: Option<Entity<Workspace>>,

    // Handles to sub-panels so we can forward notifications after scene mutations.
    hierarchy_panel_entity: Option<Entity<crate::level_editor::HierarchyPanelWrapper>>,
    properties_panel_entity: Option<Entity<crate::level_editor::PropertiesPanelWrapper>>,
}

impl LevelEditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self::new_internal(None, window, cx);
        panel.ensure_default_level_file();
        panel
    }

    pub fn new_with_window_id(window_id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self::new_internal(Some(window_id), window, cx);
        panel.ensure_default_level_file();
        panel
    }

    /// If no project is open, do nothing. Otherwise resolve `<project>/scene/default.level`:
    /// - If the file already exists, load it.
    /// - If it doesn't exist, save the current in-memory default scene to it and
    ///   set `current_scene` so the title bar and save-as shortcuts work correctly.
    fn ensure_default_level_file(&mut self) {
        let Some(project_str) = engine_state::get_project_path() else {
            return;
        };
        let default_path = std::path::PathBuf::from(&project_str)
            .join("scene")
            .join("default.level");

        if let Some(parent) = default_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    "Could not create default level directory {:?}: {e}",
                    parent
                );
                return;
            }
        }

        let scene_db = { self.shared_state.read().scene_database.clone() };

        if default_path.exists() {
            // File already on disk — load it into the shared scene db.
            scene_db.clear();
            match scene_db.load_from_file(&default_path) {
                Ok(_) => {
                    let mut w = self.shared_state.write();
                    w.current_scene = Some(default_path);
                    w.has_unsaved_changes = false;
                }
                Err(e) => {
                    tracing::warn!("Default level exists but could not be loaded: {e}");
                }
            }
        } else {
            // File does not exist — write the current default scene to disk, then set the path.
            match scene_db.save_to_file(&default_path) {
                Ok(_) => {
                    tracing::info!(
                        "Created default level file at {:?}",
                        default_path
                    );
                    let mut w = self.shared_state.write();
                    w.current_scene = Some(default_path);
                    w.has_unsaved_changes = false;
                }
                Err(e) => {
                    tracing::warn!(
                        "Could not write default level to {:?}: {e}",
                        default_path
                    );
                }
            }
        }
    }

    /// Create the editor and immediately load a level file from disk.
    ///
    /// The scene is cleared and reloaded into the existing shared `Arc<SceneDb>`
    /// so the renderer stays in sync. Returns an error string on load failure
    /// (the panel is still valid and shows the default empty scene).
    pub fn new_with_path(
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, String> {
        let mut panel = Self::new_internal(None, window, cx);
        // Clear the default scene that was just populated, then load from file.
        let scene_db = { panel.shared_state.read().scene_database.clone() };
        scene_db.clear();
        scene_db.load_from_file(&path)?;
        {
            let mut state = panel.shared_state.write();
            state.current_scene = Some(path);
            state.has_unsaved_changes = false;
        }
        Ok(panel)
    }

    fn new_internal(window_id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _horizontal_resizable_state = ResizableState::new(cx);
        let _vertical_resizable_state = ResizableState::new(cx);

        // Load engine settings for frame pacing configuration
        let settings = EngineSettings::default_path()
            .and_then(|path| Some(EngineSettings::load(&path)))
            .unwrap_or_default();

        let _max_viewport_fps = settings.advanced.max_viewport_fps;

        // Get the central GameThread from the global EngineBackend
        let (game_thread, physics_query) = match engine_backend::EngineBackend::global() {
            Some(backend_arc) => {
                let backend_guard = backend_arc.read();
                let gt = backend_guard.game_thread()
                    .expect("GameThread not initialized in EngineBackend - engine failed to initialize properly")
                    .clone();
                let pq = backend_guard.get_physics_query_service();
                (gt, pq)
            }
            None => {
                tracing::error!("EngineBackend not initialized when creating level editor");
                panic!("EngineBackend must be initialized before creating LevelEditorPanel. This is a critical engine initialization failure.");
            }
        };

        // CRITICAL: Start disabled for Edit mode (editor starts in Edit, not Play)
        game_thread.set_enabled(false);

        // Get game state reference (needed for renderer integration)
        let _game_state = game_thread.get_state();

        // Create the shared scene database FIRST so both the renderer and the UI
        // panels hold the same Arc. All reads/writes go to the same atomic storage.
        let scene_db = Arc::new(SceneDb::new());

        // Create GPU render engine sharing the scene_db Arc and physics query service
        let mut renderer_builder = GpuRendererBuilder::new(1600, 900).scene_db(scene_db.clone());
        if let Some(pq) = physics_query {
            renderer_builder = renderer_builder.physics(pq);
        }
        let gpu_engine = Arc::new(Mutex::new(renderer_builder.build()));
        let render_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Temporary debug toggle: replace viewport with a solid yellow panel to
        // verify layout/overlap issues independently of GPU rendering.
        let debug_replace_with_yellow = false;

        // Create HelioViewport — renders via WgpuSurfaceHandle every GPUI frame.
        // Must be created AFTER gpu_engine so we can share the Arc.
        let viewport =
            cx.new(|cx| HelioViewport::new(gpu_engine.clone(), debug_replace_with_yellow, cx));

        // Store GPU renderer in global EngineContext using a marker that the render loop will pick up
        // The render loop will associate it with the correct window when it first renders
        if let Some(engine_context) = engine_state::EngineContext::global() {
            if let Some(wid) = window_id {
                // We have the actual window ID - register directly!
                let handle = engine_state::TypedRendererHandle::helio(wid, gpu_engine.clone());
                engine_context.renderers.register(wid, handle);
            } else {
                // Fallback: Use a sentinel value (0) to mark this renderer as pending association with a window
                // The main render loop will detect windows with viewports and claim this renderer
                let handle = engine_state::TypedRendererHandle::helio(0, gpu_engine.clone());
                engine_context.renderers.register(0, handle);
            }
        }

        // Build the level editor state with the default scene populated into the shared SceneDb.
        // The renderer and all panels now read/write the same Arc<SceneDb>.
        let state = LevelEditorState::new_with_scene_db(scene_db);

        Self {
            focus_handle: cx.focus_handle(),
            fps_graph_is_line: Rc::new(RefCell::new(true)),
            toolbar: ToolbarPanel::new(),
            viewport,
            gpu_engine: gpu_engine.clone(),
            render_enabled,
            game_thread: game_thread.clone(),
            shared_state: Arc::new(parking_lot::RwLock::new(state)),
            workspace: None,
            hierarchy_panel_entity: None,
            properties_panel_entity: None,
        }
    }

    /// Notify the hierarchy (and, via its observer, the properties panel) so they
    /// re-render after any scene or selection mutation.
    fn notify_sub_panels(&self, cx: &mut Context<Self>) {
        if let Some(ref h) = self.hierarchy_panel_entity {
            h.update(cx, |_, cx| cx.notify());
        }
        if let Some(ref p) = self.properties_panel_entity {
            p.update(cx, |_, cx| cx.notify());
        }
    }

    fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }

        let workspace = cx.new(|cx| {
            Workspace::new_with_channel(
                "level-editor-workspace",
                ui::dock::DockChannel(3),
                window,
                cx,
            )
        });

        let shared_state = self.shared_state.clone();
        let fps_graph = self.fps_graph_is_line.clone();
        let gpu = self.gpu_engine.clone();
        let game = self.game_thread.clone();
        let viewport = self.viewport.clone();
        let render_enabled = self.render_enabled.clone();

        let (hierarchy_handle, properties_handle) = workspace.update(cx, |workspace, cx| {
            let dock_area = workspace.dock_area().downgrade();

            // Create viewport in center
            let viewport_panel_inner = ViewportPanel::new(viewport.clone(), render_enabled.clone(), window, cx);
            let viewport_panel = cx.new(|cx| {
                use crate::level_editor::ViewportPanelWrapper;
                ViewportPanelWrapper::new(
                    viewport_panel_inner,
                    shared_state.clone(),
                    fps_graph.clone(),
                    gpu.clone(),
                    game.clone(),
                    cx,
                )
            });

            // Create right dock panels
            let hierarchy_panel = cx.new(|cx| {
                use crate::level_editor::HierarchyPanelWrapper;
                HierarchyPanelWrapper::new(shared_state.clone(), cx)
            });
            let hierarchy_handle = hierarchy_panel.clone();
            let properties_panel = cx.new(|cx| {
                use crate::level_editor::PropertiesPanelWrapper;
                PropertiesPanelWrapper::new(shared_state.clone(), window, cx)
            });
            let properties_handle = properties_panel.clone();
            let world_settings_panel = cx.new(|cx| {
                use crate::level_editor::WorldSettingsPanel;
                WorldSettingsPanel::new(shared_state.clone(), window, cx)
            });

            // Wire up cross-panel notification: whenever the hierarchy is notified (e.g.
            // after a selection click), the properties panel is also notified so it
            // re-reads the selected object and updates its sections.
            {
                let hierarchy_for_observe = hierarchy_panel.clone();
                properties_panel.update(cx, |_, cx| {
                    cx.observe(&hierarchy_for_observe, |_, _, cx| {
                                                cx.notify();
                    }).detach();
                });
                            }

            // Bottom right: tabs for Properties and World Settings
            let bottom_tabs = DockItem::tabs(
                vec![
                    std::sync::Arc::new(properties_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                    std::sync::Arc::new(world_settings_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                ],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Top right: hierarchy panel (as a single-tab TabPanel)
            let top_hierarchy = DockItem::tabs(
                vec![std::sync::Arc::new(hierarchy_panel) as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Compose right dock as a vertical split: top = hierarchy (25%), bottom = tabs (75%)
            // Hierarchy gets smaller fixed size, Properties/World gets larger
            let right = ui::dock::DockItem::split_with_sizes(
                gpui::Axis::Vertical,
                vec![top_hierarchy, bottom_tabs],
                vec![Some(px(150.0)), Some(px(550.0))],  // 150px hierarchy, 550px for Properties/World
                &dock_area,
                window,
                cx,
            );

            // Set center and right dock only (no left dock, matching DAW approach)
            let center_tabs = DockItem::tabs(
                vec![std::sync::Arc::new(viewport_panel) as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );
            let _ = dock_area.update(cx, |dock_area, cx| {
                dock_area.set_center(center_tabs, window, cx);
                dock_area.set_right_dock(right, Some(px(400.0)), true, window, cx);
            });

            (hierarchy_handle, properties_handle)
        });

        self.hierarchy_panel_entity = Some(hierarchy_handle);
        self.properties_panel_entity = Some(properties_handle);
        self.workspace = Some(workspace);
    }

    pub fn toggle_rendering(&mut self) {
        let current = self
            .render_enabled
            .load(std::sync::atomic::Ordering::Relaxed);
        self.render_enabled
            .store(!current, std::sync::atomic::Ordering::Relaxed);
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.shared_state.read();
        let objects_count = state.scene_objects().len();
        let selected_name = state
            .selected_object()
            .and_then(|id| state.scene_database.get_object(&id))
            .map(|obj| obj.name.clone())
            .unwrap_or_else(|| t!("LevelEditor.StatusBar.None").to_string());

        let grid_status = if state.show_grid {
            t!("LevelEditor.StatusBar.GridOn").to_string()
        } else {
            t!("LevelEditor.StatusBar.GridOff").to_string()
        };

        let camera_mode_str = match state.camera_mode {
            CameraMode::Perspective => t!("LevelEditor.CameraMode.Perspective").to_string(),
            CameraMode::Orthographic => t!("LevelEditor.CameraMode.Orthographic").to_string(),
            CameraMode::Top => t!("LevelEditor.CameraMode.Top").to_string(),
            CameraMode::Front => t!("LevelEditor.CameraMode.Front").to_string(),
            CameraMode::Side => t!("LevelEditor.CameraMode.Side").to_string(),
        };

        let tool_name = match state.current_tool {
            TransformTool::Select => t!("LevelEditor.Tool.Select").to_string(),
            TransformTool::Move => t!("LevelEditor.Tool.Move").to_string(),
            TransformTool::Rotate => t!("LevelEditor.Tool.Rotate").to_string(),
            TransformTool::Scale => t!("LevelEditor.Tool.Scale").to_string(),
        };

        StatusBar::new()
            .add_left_item(t!("LevelEditor.StatusBar.Objects", count => objects_count).to_string())
            .add_left_item(t!("LevelEditor.StatusBar.Selected", name => &selected_name).to_string())
            .add_right_item(camera_mode_str)
            .add_right_item(grid_status)
            .add_right_item(t!("LevelEditor.StatusBar.Tool", name => &tool_name).to_string())
            .render(cx)
    }

    // Helper method to sync GPUI gizmo state to Helio's pending command queue.
    // Use a blocking lock so key-driven tool swaps are never dropped mid-frame.
    fn sync_gizmo_to_helio(&mut self) {
        if let Ok(engine) = self.gpu_engine.lock() {
            if let Some(ref helio_renderer) = engine.helio_renderer {
                let state = self.shared_state.read();

                // Map TransformTool to GizmoType for SceneDB
                use engine_backend::scene::GizmoType as SceneGizmoType;
                let gizmo_type = match state.current_tool {
                    TransformTool::Select => SceneGizmoType::None,
                    TransformTool::Move => SceneGizmoType::Translate,
                    TransformTool::Rotate => SceneGizmoType::Rotate,
                    TransformTool::Scale => SceneGizmoType::Scale,
                };
                helio_renderer.scene_db.set_gizmo_type(gizmo_type);

                // Write the new gizmo mode into the pending slot.
                // The render thread drains this at the start of the next frame — no lock contention.
                use engine_backend::GizmoMode;
                let helio_mode = match state.current_tool {
                    TransformTool::Select => GizmoMode::Translate,
                    TransformTool::Move => GizmoMode::Translate,
                    TransformTool::Rotate => GizmoMode::Rotate,
                    TransformTool::Scale => GizmoMode::Scale,
                };
                if let Ok(mut pending) = helio_renderer.pending_gizmo_mode.lock() {
                    *pending = Some(helio_mode);
                }

                tracing::info!("[GIZMO SYNC] Queued gizmo mode: {:?}", helio_mode);
            }
        }
    }

    // Queue a specific gizmo mode directly for the requested tool.
    // This avoids depending on any intermediate UI state reads.
    fn queue_gizmo_mode_for_tool(&mut self, tool: TransformTool) {
        if let Ok(engine) = self.gpu_engine.lock() {
            if let Some(ref helio_renderer) = engine.helio_renderer {
                use engine_backend::scene::GizmoType as SceneGizmoType;
                let gizmo_type = match tool {
                    TransformTool::Select => SceneGizmoType::None,
                    TransformTool::Move => SceneGizmoType::Translate,
                    TransformTool::Rotate => SceneGizmoType::Rotate,
                    TransformTool::Scale => SceneGizmoType::Scale,
                };
                helio_renderer.scene_db.set_gizmo_type(gizmo_type);

                use engine_backend::GizmoMode;
                let helio_mode = match tool {
                    TransformTool::Select => GizmoMode::Translate,
                    TransformTool::Move => GizmoMode::Translate,
                    TransformTool::Rotate => GizmoMode::Rotate,
                    TransformTool::Scale => GizmoMode::Scale,
                };
                if let Ok(mut pending) = helio_renderer.pending_gizmo_mode.lock() {
                    *pending = Some(helio_mode);
                }
            }
        }
    }

    // Action handlers
    fn on_select_tool(&mut self, _: &SelectTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().set_tool(TransformTool::Select);
        self.queue_gizmo_mode_for_tool(TransformTool::Select);
        cx.notify();
    }

    fn on_move_tool(&mut self, _: &MoveTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().set_tool(TransformTool::Move);
        self.queue_gizmo_mode_for_tool(TransformTool::Move);
        cx.notify();
    }

    fn on_rotate_tool(&mut self, _: &RotateTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().set_tool(TransformTool::Rotate);
        self.queue_gizmo_mode_for_tool(TransformTool::Rotate);
        cx.notify();
    }

    fn on_scale_tool(&mut self, _: &ScaleTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().set_tool(TransformTool::Scale);
        self.queue_gizmo_mode_for_tool(TransformTool::Scale);
        cx.notify();
    }

    // Toolbar action handlers
    fn on_set_time_scale(
        &mut self,
        action: &toolbar::SetTimeScale,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().game_time_scale = action.0;
        cx.notify();
    }

    fn on_set_multiplayer_mode(
        &mut self,
        action: &toolbar::SetMultiplayerMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().multiplayer_mode = action.0;
        cx.notify();
    }

    fn on_set_build_config(
        &mut self,
        action: &toolbar::SetBuildConfig,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build_config = action.0;
        cx.notify();
    }

    fn on_set_target_platform(
        &mut self,
        action: &toolbar::SetTargetPlatform,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().target_platform = action.0;
        cx.notify();
    }

    fn on_add_object(&mut self, _: &AddObject, _: &mut Window, cx: &mut Context<Self>) {
        let objects_count = self.shared_state.read().scene_objects().len();
        let new_object = SceneObjectData {
            id: format!("object_{}", objects_count + 1),
            name: "New Object".to_string(),
            object_type: ObjectType::Empty,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            components: vec![],
            scene_path: String::new(),
        };
        self.shared_state
            .read()
            .scene_database
            .add_object(new_object, None);
        self.shared_state.write().has_unsaved_changes = true;
        cx.notify();
    }

    fn on_add_object_of_type(
        &mut self,
        action: &AddObjectOfType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let object_type = match action.object_type.as_str() {
            "Mesh" => ObjectType::Mesh(MeshType::Cube),
            "Light" => ObjectType::Light(LightType::Directional),
            "Camera" => ObjectType::Camera,
            _ => ObjectType::Empty,
        };

        let objects_count = self.shared_state.read().scene_objects().len();
        let new_object = SceneObjectData {
            id: format!(
                "{}_{}",
                action.object_type.to_lowercase(),
                objects_count + 1
            ),
            name: format!("New {}", action.object_type),
            object_type,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            components: vec![],
            scene_path: String::new(),
        };
        self.shared_state
            .read()
            .scene_database
            .add_object(new_object, None);
        self.shared_state.write().has_unsaved_changes = true;
        self.notify_sub_panels(cx);
        cx.notify();
    }

    fn on_delete_object(&mut self, _: &DeleteObject, _: &mut Window, cx: &mut Context<Self>) {
        let selected_id = self.shared_state.read().selected_object();
        if let Some(id) = selected_id {
            self.shared_state.read().scene_database.remove_object(&id);
            self.shared_state.write().has_unsaved_changes = true;

            // Deselect after deletion
            self.shared_state.write().select_object(None);
            self.sync_gizmo_to_helio(); // Clear gizmo after deletion
        }
        self.notify_sub_panels(cx);
        cx.notify();
    }

    fn on_duplicate_object(&mut self, _: &DuplicateObject, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.shared_state.read().selected_object() {
            self.shared_state
                .read()
                .scene_database
                .duplicate_object(&id);
            self.shared_state.write().has_unsaved_changes = true;
        }
        self.notify_sub_panels(cx);
        cx.notify();
    }

    fn on_select_object(&mut self, action: &SelectObject, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .select_object(Some(action.object_id.clone()));
        self.sync_gizmo_to_helio(); // Sync gizmo to follow selected object
        self.notify_sub_panels(cx);
        cx.notify();
    }

    fn on_toggle_object_expanded(
        &mut self,
        action: &ToggleObjectExpanded,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .toggle_object_expanded(&action.object_id);
        cx.notify();
    }

    fn on_toggle_grid(&mut self, _: &ToggleGrid, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().toggle_grid();
        cx.notify();
    }

    fn on_toggle_wireframe(&mut self, _: &ToggleWireframe, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().toggle_wireframe();
        cx.notify();
    }

    fn on_toggle_lighting(&mut self, _: &ToggleLighting, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().toggle_lighting();
        cx.notify();
    }

    fn on_toggle_performance_overlay(
        &mut self,
        _: &TogglePerformanceOverlay,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_performance_overlay();
        cx.notify();
    }

    fn on_toggle_camera_mode_selector(
        &mut self,
        _: &ToggleCameraModeSelector,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_camera_mode_selector();
        cx.notify();
    }

    fn on_toggle_viewport_options(
        &mut self,
        _: &ToggleViewportOptions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_viewport_options();
        cx.notify();
    }

    fn on_toggle_fps_graph_type(
        &mut self,
        _: &ToggleFpsGraphType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_fps_graph_type();
        cx.notify();
    }

    // Performance metrics toggles
    fn on_toggle_fps_graph(&mut self, _: &ToggleFpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().toggle_fps_graph();
        cx.notify();
    }

    fn on_toggle_tps_graph(&mut self, _: &ToggleTpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().toggle_tps_graph();
        cx.notify();
    }

    fn on_toggle_frame_time_graph(
        &mut self,
        _: &ToggleFrameTimeGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_frame_time_graph();
        cx.notify();
    }

    fn on_toggle_memory_graph(
        &mut self,
        _: &ToggleMemoryGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_memory_graph();
        cx.notify();
    }

    fn on_toggle_draw_calls_graph(
        &mut self,
        _: &ToggleDrawCallsGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_draw_calls_graph();
        cx.notify();
    }

    fn on_toggle_vertices_graph(
        &mut self,
        _: &ToggleVerticesGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_vertices_graph();
        cx.notify();
    }

    fn on_toggle_input_latency_graph(
        &mut self,
        _: &ToggleInputLatencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_input_latency_graph();
        cx.notify();
    }

    fn on_toggle_ui_consistency_graph(
        &mut self,
        _: &ToggleUiConsistencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().toggle_ui_consistency_graph();
        cx.notify();
    }

    fn on_play_scene(&mut self, _: &PlayScene, _: &mut Window, cx: &mut Context<Self>) {
        // Enter play mode (saves scene snapshot)
        self.shared_state.write().enter_play_mode();

        // Enable game thread
        self.game_thread.set_enabled(true);

        // Disable gizmos in play mode
        self.sync_gizmo_to_helio();

        cx.notify();
    }

    fn on_stop_scene(&mut self, _: &StopScene, _: &mut Window, cx: &mut Context<Self>) {
        // Disable game thread
        self.game_thread.set_enabled(false);

        // Exit play mode (restores scene from snapshot)
        self.shared_state.write().exit_play_mode();

        // Re-enable gizmos in edit mode
        self.sync_gizmo_to_helio();

        cx.notify();
    }

    fn on_perspective_view(&mut self, _: &PerspectiveView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .set_camera_mode(CameraMode::Perspective);
        cx.notify();
    }

    fn on_orthographic_view(
        &mut self,
        _: &OrthographicView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .set_camera_mode(CameraMode::Orthographic);
        cx.notify();
    }

    fn on_top_view(&mut self, _: &TopView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().set_camera_mode(CameraMode::Top);
        cx.notify();
    }

    fn on_front_view(&mut self, _: &FrontView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().set_camera_mode(CameraMode::Front);
        cx.notify();
    }

    fn on_side_view(&mut self, _: &SideView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().set_camera_mode(CameraMode::Side);
        cx.notify();
    }

    fn on_save_scene(&mut self, _: &SaveScene, _: &mut Window, cx: &mut Context<Self>) {
        // If no current scene path, do Save As
        if self.shared_state.read().current_scene.is_none() {
            cx.dispatch_action(&SaveSceneAs);
            return;
        }

        let (scene_db, path_opt) = {
            let state = self.shared_state.read();
            (state.scene_database.clone(), state.current_scene.clone())
        };

        if let Some(path) = path_opt {
            match scene_db.save_to_file(&path) {
                Ok(_) => {
                    self.shared_state.write().has_unsaved_changes = false;
                    cx.notify();
                }
                Err(e) => {}
            }
        }
    }

    fn on_save_scene_as(&mut self, _: &SaveSceneAs, _window: &mut Window, cx: &mut Context<Self>) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene_database.clone() };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Save Scene As")
            .add_filter("Level file", &["level", "json"])
            .set_file_name("untitled.level");
        cx.spawn(async move |this, cx| {
            if let Some(handle) = dialog.save_file().await {
                let path = handle.path().to_path_buf();
                let result = scene_db.save_to_file(&path);
                cx.update(|cx| {
                    this.update(cx, |_, cx| {
                        match result {
                            Ok(_) => {
                                state_arc.write().current_scene = Some(path);
                                state_arc.write().has_unsaved_changes = false;
                            }
                            Err(e) => tracing::error!("Save failed: {}", e),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
            }
        })
        .detach();
    }

    fn on_open_scene(&mut self, _: &OpenScene, _window: &mut Window, cx: &mut Context<Self>) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene_database.clone() };
        let default_dir = state_arc
            .read()
            .current_scene
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Open Scene")
            .add_filter("Level file", &["level", "json"])
            .set_directory(default_dir);
        cx.spawn(async move |this, cx| {
            if let Some(handle) = dialog.pick_file().await {
                let path = handle.path().to_path_buf();
                // Load into the existing shared SceneDb (renderer keeps its Arc).
                let result = scene_db.load_from_file(&path);
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        match result {
                            Ok(_) => {
                                let mut state = state_arc.write();
                                state.current_scene = Some(path);
                                state.has_unsaved_changes = false;
                                // Deselect so properties panel clears stale data.
                                state.select_object(None);
                            }
                            Err(e) => tracing::error!("Open scene failed: {}", e),
                        }
                        this.notify_sub_panels(cx);
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
            }
        })
        .detach();
    }

    fn on_new_scene(&mut self, _: &NewScene, _: &mut Window, cx: &mut Context<Self>) {
        // Warn if unsaved changes (TODO: modal dialog)
        // Clear the scene IN-PLACE so the renderer keeps its Arc<SceneDb>.
        {
            let state = self.shared_state.read();
            state.scene_database.clear();
            // Re-populate defaults into the same shared SceneDb.
            state.scene_database.populate_default_scene_pub();
        }
        self.notify_sub_panels(cx);
        {
            let mut state = self.shared_state.write();
            state.current_scene = None;
            state.has_unsaved_changes = false;
            // Deselect so properties panel clears stale data.
            state.select_object(None);
        }
        cx.notify();
    }

    fn on_toggle_snapping(&mut self, _: &ToggleSnapping, _: &mut Window, cx: &mut Context<Self>) {
        // Toggle snapping in gizmo state
        let state = self.shared_state.read();
        let mut gizmo_state = state.gizmo_state.write();
        gizmo_state.toggle_snap();
        let _enabled = gizmo_state.snap_enabled;
        let _increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_toggle_local_space(
        &mut self,
        _: &ToggleLocalSpace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Toggle local/world space in gizmo state
        let state = self.shared_state.read();
        let mut gizmo_state = state.gizmo_state.write();
        gizmo_state.toggle_space();
        let _is_local = gizmo_state.local_space;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_increase_snap_increment(
        &mut self,
        _: &IncreaseSnapIncrement,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.shared_state.read();
        let mut gizmo_state = state.gizmo_state.write();
        // Double the snap increment (0.25, 0.5, 1.0, 2.0, 4.0, etc.)
        gizmo_state.snap_increment = (gizmo_state.snap_increment * 2.0).min(10.0);
        let _increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_decrease_snap_increment(
        &mut self,
        _: &DecreaseSnapIncrement,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.shared_state.read();
        let mut gizmo_state = state.gizmo_state.write();
        // Halve the snap increment (10.0, 5.0, 2.5, 1.0, 0.5, 0.25, etc.)
        gizmo_state.snap_increment = (gizmo_state.snap_increment / 2.0).max(0.1);
        let _increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_focus_selected(
        &mut self,
        _: &FocusSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // TODO: Frame selected object in viewport (move camera to focus on selection)
        if let Some(_obj) = self.shared_state.read().get_selected_object() {
            // For now just log - implementing camera movement would require Bevy camera manipulation
        }
        cx.notify();
    }
}

impl Panel for LevelEditorPanel {
    fn panel_name(&self) -> &'static str {
        "Level Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div()
            .child(
                if let Some(ref scene) = self.shared_state.read().current_scene {
                    format!(
                        "Level Editor - {}{}",
                        scene
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Untitled"),
                        if self.shared_state.write().has_unsaved_changes {
                            " *"
                        } else {
                            ""
                        }
                    )
                } else {
                    "Level Editor".to_string()
                },
            )
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}

ui_common::panel_boilerplate!(LevelEditorPanel);

impl EventEmitter<PanelEvent> for LevelEditorPanel {}

impl Render for LevelEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Initialize workspace on first render
        self.initialize_workspace(window, cx);

        // Sync selection and gizmo tool state to the backend SceneDB every frame.
        if let Ok(engine_guard) = self.gpu_engine.try_lock() {
            if let Some(ref helio_renderer) = engine_guard.helio_renderer {
                let state = self.shared_state.read();
                let gpui_selected = state.selected_object();

                use engine_backend::scene::GizmoType as SceneGizmoType;
                let gizmo_type = match state.current_tool {
                    TransformTool::Select => SceneGizmoType::None,
                    TransformTool::Move => SceneGizmoType::Translate,
                    TransformTool::Rotate => SceneGizmoType::Rotate,
                    TransformTool::Scale => SceneGizmoType::Scale,
                };
                drop(state);

                if gpui_selected != helio_renderer.scene_db.get_selected_id() {
                    helio_renderer.scene_db.select_object(gpui_selected.clone());
                }
                if gizmo_type != helio_renderer.scene_db.get_gizmo_state().gizmo_type {
                    helio_renderer.scene_db.set_gizmo_type(gizmo_type);
                }
            }
        }

        v_flex()
            .size_full()
            // NO BACKGROUND - allow transparency for viewport
            .key_context("LevelEditor")
            .track_focus(&self.focus_handle)
            // Scene operations
            .on_action(cx.listener(Self::on_new_scene))
            .on_action(cx.listener(Self::on_open_scene))
            .on_action(cx.listener(Self::on_save_scene))
            .on_action(cx.listener(Self::on_save_scene_as))
            // Transform tools - KEYBOARD: Q/W/E/R
            .on_action(cx.listener(Self::on_select_tool))
            .on_action(cx.listener(Self::on_move_tool))
            .on_action(cx.listener(Self::on_rotate_tool))
            .on_action(cx.listener(Self::on_scale_tool))
            // Toolbar actions
            .on_action(cx.listener(Self::on_set_time_scale))
            .on_action(cx.listener(Self::on_set_multiplayer_mode))
            .on_action(cx.listener(Self::on_set_build_config))
            .on_action(cx.listener(Self::on_set_target_platform))
            // Object operations
            .on_action(cx.listener(Self::on_add_object))
            .on_action(cx.listener(Self::on_add_object_of_type))
            .on_action(cx.listener(Self::on_delete_object))
            .on_action(cx.listener(Self::on_duplicate_object))
            .on_action(cx.listener(Self::on_select_object))
            .on_action(cx.listener(Self::on_toggle_object_expanded))
            .on_action(cx.listener(Self::on_focus_selected))
            // Gizmo operations - KEYBOARD: G/L/[/]
            .on_action(cx.listener(Self::on_toggle_snapping))
            .on_action(cx.listener(Self::on_toggle_local_space))
            .on_action(cx.listener(Self::on_increase_snap_increment))
            .on_action(cx.listener(Self::on_decrease_snap_increment))
            // View operations
            .on_action(cx.listener(Self::on_toggle_grid))
            .on_action(cx.listener(Self::on_toggle_wireframe))
            .on_action(cx.listener(Self::on_toggle_lighting))
            .on_action(cx.listener(Self::on_toggle_performance_overlay))
            .on_action(cx.listener(Self::on_toggle_camera_mode_selector))
            .on_action(cx.listener(Self::on_toggle_viewport_options))
            .on_action(cx.listener(Self::on_toggle_fps_graph_type))
            // Performance metrics toggles
            .on_action(cx.listener(Self::on_toggle_fps_graph))
            .on_action(cx.listener(Self::on_toggle_tps_graph))
            .on_action(cx.listener(Self::on_toggle_frame_time_graph))
            .on_action(cx.listener(Self::on_toggle_memory_graph))
            .on_action(cx.listener(Self::on_toggle_draw_calls_graph))
            .on_action(cx.listener(Self::on_toggle_vertices_graph))
            .on_action(cx.listener(Self::on_toggle_input_latency_graph))
            .on_action(cx.listener(Self::on_toggle_ui_consistency_graph))
            // Play/Edit mode
            .on_action(cx.listener(Self::on_play_scene))
            .on_action(cx.listener(Self::on_stop_scene))
            // Camera modes
            .on_action(cx.listener(Self::on_perspective_view))
            .on_action(cx.listener(Self::on_orthographic_view))
            .on_action(cx.listener(Self::on_top_view))
            .on_action(cx.listener(Self::on_front_view))
            .on_action(cx.listener(Self::on_side_view))
            // Keyboard shortcuts - LETTER KEYS for fast workflow
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Respond if this panel or any child (e.g. viewport) has focus,
                // and no modifier keys are held.
                if !this.focus_handle.contains_focused(window, cx)
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                    || event.keystroke.modifiers.shift
                    || event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.function
                {
                    return;
                }

                match event.keystroke.key.as_ref() {
                    "escape" => {
                        // Update UI state unconditionally — always clear GPUI selection.
                        this.shared_state.write().select_object(None);
                        // Signal the render thread to call editor_state.deselect() next frame.
                        // This is written directly to the Arc<AtomicBool> — no engine lock needed.
                        if let Ok(engine) = this.gpu_engine.lock() {
                            if let Some(ref helio_renderer) = engine.helio_renderer {
                                use std::sync::atomic::Ordering;
                                helio_renderer
                                    .pending_deselect
                                    .store(true, Ordering::Release);
                            }
                        }
                        cx.notify();
                    }
                    // Tool selection — call handlers directly to avoid action-dispatch drift.
                    "q" => this.on_select_tool(&SelectTool, window, cx),
                    "w" => this.on_move_tool(&MoveTool, window, cx),
                    "g" => this.on_move_tool(&MoveTool, window, cx), // Blender: G = Grab/Move
                    "e" => this.on_rotate_tool(&RotateTool, window, cx),
                    "r" => this.on_rotate_tool(&RotateTool, window, cx), // Blender: R = Rotate
                    "s" => this.on_scale_tool(&ScaleTool, window, cx),   // Blender: S = Scale
                    "l" => cx.dispatch_action(&ToggleLocalSpace),
                    "f" => cx.dispatch_action(&FocusSelected),
                    "[" => cx.dispatch_action(&DecreaseSnapIncrement),
                    "]" => cx.dispatch_action(&IncreaseSnapIncrement),
                    _ => {}
                }
            }))
            // Additional keyboard shortcuts for Alt+Up/Down (object reordering)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Only respond if Alt key is pressed and this panel or any child has focus
                if !this.focus_handle.contains_focused(window, cx) || !event.keystroke.modifiers.alt
                {
                    return;
                }

                match event.keystroke.key.as_ref() {
                    "up" => {
                        // Move selected object up in hierarchy
                        if let Some(id) = this.shared_state.read().selected_object() {
                            this.shared_state.read().scene_database.move_object_up(&id);
                            cx.notify();
                        }
                    }
                    "down" => {
                        // Move selected object down in hierarchy
                        if let Some(id) = this.shared_state.read().selected_object() {
                            this.shared_state
                                .read()
                                .scene_database
                                .move_object_down(&id);
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .child(
                // Toolbar at the top
                self.toolbar.render(
                    &*self.shared_state.read(),
                    self.shared_state.clone(),
                    self.gpu_engine.clone(),
                    cx,
                ),
            )
            .child(
                // Workspace with draggable panels
                if let Some(ref workspace) = self.workspace {
                    workspace.clone().into_any_element()
                } else {
                    div().child("Loading workspace...").into_any_element()
                },
            )
            .child(
                // Status bar at the bottom
                self.render_status_bar(cx),
            )
    }
}
