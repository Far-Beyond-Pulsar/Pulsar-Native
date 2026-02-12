use gpui::*;
use rust_i18n::t;
use ui::{
    dock::{Panel, PanelEvent, DockItem},
    workspace::Workspace,
    resizable::ResizableState,
    v_flex,
};
// Zero-copy Bevy viewport for 3D rendering
use ui::bevy_viewport::{BevyViewport, BevyViewportState};

use ui::settings::EngineSettings;
use engine_backend::services::gpu_renderer::GpuRenderer;
use ui_common::StatusBar;
use engine_backend::GameThread;
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;

use super::{
    LevelEditorState,
    ViewportPanel, ToolbarPanel, CameraMode, TransformTool, toolbar,
};
use super::actions::*;
use crate::level_editor::scene_database::{
    SceneObjectData, ObjectType, Transform, MeshType, LightType,
};

/// Main Level Editor Panel - Orchestrates all sub-components
pub struct LevelEditorPanel {
    focus_handle: FocusHandle,

    // Shared state
    state: LevelEditorState,
    
    // FPS graph type state (shared with viewport for Switch)
    fps_graph_is_line: Rc<RefCell<bool>>,

    // UI Components
    toolbar: ToolbarPanel,

    // Zero-copy Bevy viewport for 3D rendering
    viewport: Entity<BevyViewport>,
    viewport_state: Arc<parking_lot::RwLock<BevyViewportState>>,
    gpu_engine: Arc<Mutex<GpuRenderer>>, // Full GPU renderer from backend
    render_enabled: Arc<std::sync::atomic::AtomicBool>,
    
    // Game thread for object movement and game logic
    game_thread: Arc<GameThread>,
    
    // Shared state for panels
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    
    // Workspace for draggable panels
    workspace: Option<Entity<Workspace>>,
}

impl LevelEditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_internal(None, window, cx)
    }

    pub fn new_with_window_id(window_id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_internal(Some(window_id), window, cx)
    }

    fn new_internal(window_id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _horizontal_resizable_state = ResizableState::new(cx);
        let _vertical_resizable_state = ResizableState::new(cx);

        // Load engine settings for frame pacing configuration
        let settings = EngineSettings::default_path()
            .and_then(|path| Some(EngineSettings::load(&path)))
            .unwrap_or_default();

        let _max_viewport_fps = settings.advanced.max_viewport_fps;

        // Create Bevy viewport with zero-copy shared textures
        let viewport = cx.new(|cx| BevyViewport::new(1600, 900, cx));
        let viewport_state = viewport.read(cx).shared_state();
        
        // Get the central GameThread from the global EngineBackend
        let game_thread = if let Some(backend) = engine_backend::EngineBackend::global() {
            let backend_guard = backend.read();
            backend_guard.game_thread()
                .expect("GameThread not initialized in EngineBackend")
                .clone()
        } else {
            panic!("EngineBackend not initialized! Cannot create level editor viewport.");
        };

        // CRITICAL: Start disabled for Edit mode (editor starts in Edit, not Play)
        game_thread.set_enabled(false);

        // Get game state reference (needed for renderer integration)
        let _game_state = game_thread.get_state();
        
        // Create GPU render engine WITHOUT game thread link initially (Edit mode)
        let gpu_engine = Arc::new(Mutex::new(GpuRenderer::new(1600, 900))); // No game state initially
        let render_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Store GPU renderer in global EngineContext using a marker that the render loop will pick up
        // The render loop will associate it with the correct window when it first renders
        if let Some(engine_context) = engine_state::EngineContext::global() {
            if let Some(wid) = window_id {
                // We have the actual window ID - register directly!
                let handle = engine_state::TypedRendererHandle::bevy(wid, gpu_engine.clone());
                engine_context.renderers.register(wid, handle);
            } else {
                // Fallback: Use a sentinel value (0) to mark this renderer as pending association with a window
                // The main render loop will detect windows with viewports and claim this renderer
                let handle = engine_state::TypedRendererHandle::bevy(0, gpu_engine.clone());
                engine_context.renderers.register(0, handle);
            }
        } else {
            tracing::debug!("[LEVEL-EDITOR] ❌ ERROR: No global EngineContext found!");
        }

        // Viewport stays transparent - Bevy renders directly to winit back buffer BEHIND GPUI
        // No texture initialization needed in GPUI - all handled in main.rs rendering loop
        tracing::debug!("[LEVEL-EDITOR] 📺 Viewport configured as transparent (Bevy renders to back buffer)");

        
        tracing::debug!("[LEVEL-EDITOR] Modular level editor initialized");

        let state = LevelEditorState::new();
        
        Self {
            focus_handle: cx.focus_handle(),
            state: LevelEditorState::new(), // Will be moved to shared_state
            fps_graph_is_line: Rc::new(RefCell::new(true)),  // Default to line graph
            toolbar: ToolbarPanel::new(),
            viewport,
            viewport_state,
            gpu_engine: gpu_engine.clone(),
            render_enabled,
            game_thread: game_thread.clone(),
            shared_state: Arc::new(parking_lot::RwLock::new(state)),
            workspace: None,
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
                cx
            )
        });

        let shared_state = self.shared_state.clone();
        let fps_graph = self.fps_graph_is_line.clone();
        let gpu = self.gpu_engine.clone();
        let game = self.game_thread.clone();
        let viewport = self.viewport.clone();
        let render_enabled = self.render_enabled.clone();

        workspace.update(cx, |workspace, cx| {
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
            let properties_panel = cx.new(|cx| {
                use crate::level_editor::PropertiesPanelWrapper;
                PropertiesPanelWrapper::new(shared_state.clone(), window, cx)
            });
            let world_settings_panel = cx.new(|cx| {
                use crate::level_editor::WorldSettingsPanel;
                WorldSettingsPanel::new(shared_state.clone(), window, cx)
            });

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
        });

        self.workspace = Some(workspace);
    }

    pub fn toggle_rendering(&mut self) {
        let current = self.render_enabled.load(std::sync::atomic::Ordering::Relaxed);
        self.render_enabled.store(!current, std::sync::atomic::Ordering::Relaxed);
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let objects_count = self.state.scene_objects().len();
        let selected_name = self.state.selected_object()
            .and_then(|id| self.state.scene_database.get_object(&id))
            .map(|obj| obj.name.clone())
            .unwrap_or_else(|| t!("LevelEditor.StatusBar.None").to_string());
        
        let grid_status = if self.state.show_grid {
            t!("LevelEditor.StatusBar.GridOn").to_string()
        } else {
            t!("LevelEditor.StatusBar.GridOff").to_string()
        };
        
        let camera_mode_str = match self.state.camera_mode {
            CameraMode::Perspective => t!("LevelEditor.CameraMode.Perspective").to_string(),
            CameraMode::Orthographic => t!("LevelEditor.CameraMode.Orthographic").to_string(),
            CameraMode::Top => t!("LevelEditor.CameraMode.Top").to_string(),
            CameraMode::Front => t!("LevelEditor.CameraMode.Front").to_string(),
            CameraMode::Side => t!("LevelEditor.CameraMode.Side").to_string(),
        };
        
        let tool_name = match self.state.current_tool {
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

    // Helper method to sync GPUI gizmo state to Bevy's shared resource
    fn sync_gizmo_to_bevy(&mut self) {
        if let Ok(engine) = self.gpu_engine.try_lock() {
            if let Some(ref helio_renderer) = engine.helio_renderer {
                if let Ok(mut bevy_gizmo) = helio_renderer.gizmo_state.try_lock() {
                    let gpui_gizmo = self.state.gizmo_state.read();

                    // Map GPUI GizmoType to Bevy GizmoType
                    use engine_backend::subsystems::render::helio_renderer::BevyGizmoType;
                    use crate::level_editor::GizmoType;
                    bevy_gizmo.gizmo_type = match gpui_gizmo.gizmo_type {
                        GizmoType::None => BevyGizmoType::None,
                        GizmoType::Translate => BevyGizmoType::Translate,
                        GizmoType::Rotate => BevyGizmoType::Rotate,
                        GizmoType::Scale => BevyGizmoType::Scale,
                    };

                    // Sync selected object and position
                    if let Some(ref target_id) = gpui_gizmo.target_object_id {
                        bevy_gizmo.selected_object_id = Some(target_id.clone());

                        // Update gizmo position from selected object's transform
                        if let Some(obj) = self.state.scene_database.get_object(target_id) {
                            bevy_gizmo.target_position.x = obj.transform.position[0];
                            bevy_gizmo.target_position.y = obj.transform.position[1];
                            bevy_gizmo.target_position.z = obj.transform.position[2];
                        }
                    } else {
                        bevy_gizmo.selected_object_id = None;
                    }

                    // Sync editor mode
                    bevy_gizmo.enabled = self.state.is_edit_mode();
                }
            }
        }
    }

    // Action handlers
    fn on_select_tool(&mut self, _: &SelectTool, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_tool(TransformTool::Select);
        self.sync_gizmo_to_bevy();
        cx.notify();
    }

    fn on_move_tool(&mut self, _: &MoveTool, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_tool(TransformTool::Move);
        self.sync_gizmo_to_bevy();
        cx.notify();
    }

    fn on_rotate_tool(&mut self, _: &RotateTool, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_tool(TransformTool::Rotate);
        self.sync_gizmo_to_bevy();
        cx.notify();
    }

    fn on_scale_tool(&mut self, _: &ScaleTool, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_tool(TransformTool::Scale);
        self.sync_gizmo_to_bevy();
        cx.notify();
    }

    // Toolbar action handlers
    fn on_set_time_scale(&mut self, action: &toolbar::SetTimeScale, _: &mut Window, cx: &mut Context<Self>) {
        self.state.game_time_scale = action.0;
        cx.notify();
    }

    fn on_set_multiplayer_mode(&mut self, action: &toolbar::SetMultiplayerMode, _: &mut Window, cx: &mut Context<Self>) {
        self.state.multiplayer_mode = action.0;
        cx.notify();
    }

    fn on_set_build_config(&mut self, action: &toolbar::SetBuildConfig, _: &mut Window, cx: &mut Context<Self>) {
        self.state.build_config = action.0;
        cx.notify();
    }

    fn on_set_target_platform(&mut self, action: &toolbar::SetTargetPlatform, _: &mut Window, cx: &mut Context<Self>) {
        self.state.target_platform = action.0;
        cx.notify();
    }

    fn on_add_object(&mut self, _: &AddObject, _: &mut Window, cx: &mut Context<Self>) {
        let objects_count = self.state.scene_objects().len();
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
        };
        self.state.scene_database.add_object(new_object, None);
        self.state.has_unsaved_changes = true;
        cx.notify();
    }

    fn on_add_object_of_type(&mut self, action: &AddObjectOfType, _: &mut Window, cx: &mut Context<Self>) {
        let object_type = match action.object_type.as_str() {
            "Mesh" => ObjectType::Mesh(MeshType::Cube),
            "Light" => ObjectType::Light(LightType::Directional),
            "Camera" => ObjectType::Camera,
            _ => ObjectType::Empty,
        };

        let objects_count = self.state.scene_objects().len();
        let new_object = SceneObjectData {
            id: format!("{}_{}", action.object_type.to_lowercase(), objects_count + 1),
            name: format!("New {}", action.object_type),
            object_type,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            components: vec![],
        };
        self.state.scene_database.add_object(new_object, None);
        self.state.has_unsaved_changes = true;
        cx.notify();
    }

    fn on_delete_object(&mut self, _: &DeleteObject, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.state.selected_object() {
            self.state.scene_database.remove_object(&id);
            self.state.has_unsaved_changes = true;

            // Deselect after deletion
            self.state.select_object(None);
            self.sync_gizmo_to_bevy(); // Clear gizmo after deletion
        }
        cx.notify();
    }

    fn on_duplicate_object(&mut self, _: &DuplicateObject, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.state.selected_object() {
            self.state.scene_database.duplicate_object(&id);
            self.state.has_unsaved_changes = true;
        }
        cx.notify();
    }

    fn on_select_object(&mut self, action: &SelectObject, _: &mut Window, cx: &mut Context<Self>) {
        self.state.select_object(Some(action.object_id.clone()));
        self.sync_gizmo_to_bevy(); // Sync gizmo to follow selected object
        cx.notify();
    }

    fn on_toggle_object_expanded(&mut self, action: &ToggleObjectExpanded, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_object_expanded(&action.object_id);
        cx.notify();
    }

    fn on_toggle_grid(&mut self, _: &ToggleGrid, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_grid();
        cx.notify();
    }

    fn on_toggle_wireframe(&mut self, _: &ToggleWireframe, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_wireframe();
        cx.notify();
    }

    fn on_toggle_lighting(&mut self, _: &ToggleLighting, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_lighting();
        cx.notify();
    }

    fn on_toggle_performance_overlay(&mut self, _: &TogglePerformanceOverlay, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_performance_overlay();
        cx.notify();
    }

    fn on_toggle_camera_mode_selector(&mut self, _: &ToggleCameraModeSelector, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_camera_mode_selector();
        cx.notify();
    }

    fn on_toggle_viewport_options(&mut self, _: &ToggleViewportOptions, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_viewport_options();
        cx.notify();
    }

    fn on_toggle_fps_graph_type(&mut self, _: &ToggleFpsGraphType, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_fps_graph_type();
        cx.notify();
    }

    // Performance metrics toggles
    fn on_toggle_fps_graph(&mut self, _: &ToggleFpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_fps_graph();
        cx.notify();
    }

    fn on_toggle_tps_graph(&mut self, _: &ToggleTpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_tps_graph();
        cx.notify();
    }

    fn on_toggle_frame_time_graph(&mut self, _: &ToggleFrameTimeGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_frame_time_graph();
        cx.notify();
    }

    fn on_toggle_memory_graph(&mut self, _: &ToggleMemoryGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_memory_graph();
        cx.notify();
    }

    fn on_toggle_draw_calls_graph(&mut self, _: &ToggleDrawCallsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_draw_calls_graph();
        cx.notify();
    }

    fn on_toggle_vertices_graph(&mut self, _: &ToggleVerticesGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_vertices_graph();
        cx.notify();
    }

    fn on_toggle_input_latency_graph(&mut self, _: &ToggleInputLatencyGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_input_latency_graph();
        cx.notify();
    }

    fn on_toggle_ui_consistency_graph(&mut self, _: &ToggleUiConsistencyGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_ui_consistency_graph();
        cx.notify();
    }

    fn on_play_scene(&mut self, _: &PlayScene, _: &mut Window, cx: &mut Context<Self>) {
        // Enter play mode (saves scene snapshot)
        self.state.enter_play_mode();

        // Enable game thread
        self.game_thread.set_enabled(true);

        // Disable gizmos in play mode
        self.sync_gizmo_to_bevy();

        cx.notify();
    }

    fn on_stop_scene(&mut self, _: &StopScene, _: &mut Window, cx: &mut Context<Self>) {
        // Disable game thread
        self.game_thread.set_enabled(false);

        // Exit play mode (restores scene from snapshot)
        self.state.exit_play_mode();

        // Re-enable gizmos in edit mode
        self.sync_gizmo_to_bevy();

        cx.notify();
    }

    fn on_perspective_view(&mut self, _: &PerspectiveView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_camera_mode(CameraMode::Perspective);
        cx.notify();
    }

    fn on_orthographic_view(&mut self, _: &OrthographicView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_camera_mode(CameraMode::Orthographic);
        cx.notify();
    }

    fn on_top_view(&mut self, _: &TopView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_camera_mode(CameraMode::Top);
        cx.notify();
    }

    fn on_front_view(&mut self, _: &FrontView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_camera_mode(CameraMode::Front);
        cx.notify();
    }

    fn on_side_view(&mut self, _: &SideView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.set_camera_mode(CameraMode::Side);
        cx.notify();
    }
    
    fn on_save_scene(&mut self, _: &SaveScene, _: &mut Window, cx: &mut Context<Self>) {
        // If no current scene path, do Save As
        if self.state.current_scene.is_none() {
            cx.dispatch_action(&SaveSceneAs);
            return;
        }
        
        if let Some(ref path) = self.state.current_scene {
            match self.state.scene_database.save_to_file(path) {
                Ok(_) => {
                    tracing::debug!("[LEVEL-EDITOR] 💾 Scene saved: {:?}", path);
                    self.state.has_unsaved_changes = false;
                    cx.notify();
                }
                Err(e) => {
                    tracing::debug!("[LEVEL-EDITOR] ❌ Failed to save scene: {}", e);
                }
            }
        }
    }
    
    fn on_save_scene_as(&mut self, _: &SaveSceneAs, _window: &mut Window, cx: &mut Context<Self>) {
        // TODO: Implement async file dialog
        tracing::debug!("[LEVEL-EDITOR] 💾 Save Scene As - TODO");
        if let Some(ref path) = self.state.current_scene {
            match self.state.scene_database.save_to_file(path) {
                Ok(_) => {
                    tracing::debug!("[LEVEL-EDITOR] 💾 Scene saved: {:?}", path);
                    self.state.has_unsaved_changes = false;
                    cx.notify();
                }
                Err(e) => {
                    tracing::debug!("[LEVEL-EDITOR] ❌ Failed to save scene: {}", e);
                }
            }
        }
    }
    
    fn on_open_scene(&mut self, _: &OpenScene, _window: &mut Window, cx: &mut Context<Self>) {
        // TODO: Implement async file dialog
        tracing::debug!("[LEVEL-EDITOR] 📂 Open Scene - TODO");
        cx.notify();
    }
    
    fn on_new_scene(&mut self, _: &NewScene, _: &mut Window, cx: &mut Context<Self>) {
        // Warn if unsaved changes
        if self.state.has_unsaved_changes {
            // TODO: Show confirmation dialog
        }
        
        // Clear scene and reset to defaults
        self.state.scene_database.clear();
        self.state.current_scene = None;
        self.state.has_unsaved_changes = false;
        
        // Re-add default objects
        self.state.scene_database = crate::level_editor::SceneDatabase::with_default_scene();
        
        tracing::debug!("[LEVEL-EDITOR] 📄 New scene created");
        cx.notify();
    }

    fn on_toggle_snapping(&mut self, _: &ToggleSnapping, _: &mut Window, cx: &mut Context<Self>) {
        // Toggle snapping in gizmo state
        let mut gizmo_state = self.state.gizmo_state.write();
        gizmo_state.toggle_snap();
        let _enabled = gizmo_state.snap_enabled;
        let _increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_toggle_local_space(&mut self, _: &ToggleLocalSpace, _: &mut Window, cx: &mut Context<Self>) {
        // Toggle local/world space in gizmo state
        let mut gizmo_state = self.state.gizmo_state.write();
        gizmo_state.toggle_space();
        let _is_local = gizmo_state.local_space;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_increase_snap_increment(&mut self, _: &IncreaseSnapIncrement, _: &mut Window, cx: &mut Context<Self>) {
        let mut gizmo_state = self.state.gizmo_state.write();
        // Double the snap increment (0.25, 0.5, 1.0, 2.0, 4.0, etc.)
        gizmo_state.snap_increment = (gizmo_state.snap_increment * 2.0).min(10.0);
        let _increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_decrease_snap_increment(&mut self, _: &DecreaseSnapIncrement, _: &mut Window, cx: &mut Context<Self>) {
        let mut gizmo_state = self.state.gizmo_state.write();
        // Halve the snap increment (10.0, 5.0, 2.5, 1.0, 0.5, 0.25, etc.)
        gizmo_state.snap_increment = (gizmo_state.snap_increment / 2.0).max(0.1);
        let _increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_focus_selected(&mut self, _: &FocusSelected, window: &mut Window, cx: &mut Context<Self>) {
        // TODO: Frame selected object in viewport (move camera to focus on selection)
        if let Some(_obj) = self.state.get_selected_object() {
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
                if let Some(ref scene) = self.state.current_scene {
                    format!(
                        "Level Editor - {}{}",
                        scene.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Untitled"),
                        if self.state.has_unsaved_changes { " *" } else { "" }
                    )
                } else {
                    "Level Editor".to_string()
                }
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

impl Focusable for LevelEditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for LevelEditorPanel {}

impl Render for LevelEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Initialize workspace on first render
        self.initialize_workspace(window, cx);
        
        // Sync viewport buffer index with Bevy's read index each frame
        // ALSO sync selection state from Bevy back to GPUI
        if let Ok(engine_guard) = self.gpu_engine.try_lock() {
            if let Some(ref helio_renderer) = engine_guard.helio_renderer {
                let read_idx = helio_renderer.get_read_index();
                let mut state = self.viewport_state.write();
                state.set_active_buffer(read_idx);

                // BIDIRECTIONAL SYNC: Poll Bevy's gizmo state for selection changes
                if let Ok(bevy_gizmo) = helio_renderer.gizmo_state.try_lock() {
                    let bevy_selected_id = bevy_gizmo.selected_object_id.clone();
                    drop(bevy_gizmo); // Release lock immediately
                    
                    // Check if Bevy's selection differs from GPUI's
                    let gpui_selected_id = self.shared_state.read().selected_object();
                    
                    if bevy_selected_id != gpui_selected_id {
                        // Bevy has a different selection - sync to GPUI!
                        if let Some(ref new_id) = bevy_selected_id {
                            tracing::debug!("[LEVEL-EDITOR] 🔄 Syncing selection from Bevy: {}", new_id);
                            self.shared_state.write().select_object(Some(new_id.clone()));
                            cx.notify(); // Trigger UI update
                        } else {
                            // Bevy deselected
                            tracing::debug!("[LEVEL-EDITOR] 🔄 Syncing deselection from Bevy");
                            self.shared_state.write().select_object(None);
                            cx.notify();
                        }
                    }
                }
                
                // TRANSFORM SYNC: Poll for transform updates from Bevy (e.g., from gizmo dragging)
                if let Ok(bevy_gizmo) = helio_renderer.gizmo_state.try_lock() {
                    // Handle transform updates from gizmo (disabled - gizmos removed)
                    if let Some(ref _updated_id) = bevy_gizmo.updated_object_id {
                        // Gizmo interaction is disabled
                    }
                }
            }
        }

        // Request continuous animation frames to keep viewport and stats updating
        // This creates a render loop synchronized with the display refresh rate
        //TODO: Optimize to only request when necessary (The viewport is now a transparent hole son continuous updates may not be needed for the entire window)
        window.request_animation_frame();

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
                // Only respond if this panel has focus and keys are unmodified
                if !this.focus_handle.is_focused(window)
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                    || event.keystroke.modifiers.shift
                    || event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.function
                {
                    return;
                }
                
                match event.keystroke.key.as_ref() {
                    "q" => cx.dispatch_action(&SelectTool),
                    "w" => cx.dispatch_action(&MoveTool),
                    "e" => cx.dispatch_action(&RotateTool),
                    "r" => cx.dispatch_action(&ScaleTool),
                    "g" => cx.dispatch_action(&ToggleSnapping),
                    "l" => cx.dispatch_action(&ToggleLocalSpace),
                    "f" => cx.dispatch_action(&FocusSelected),
                    "[" => cx.dispatch_action(&DecreaseSnapIncrement),
                    "]" => cx.dispatch_action(&IncreaseSnapIncrement),
                    _ => {}
                }
            }))
            // Additional keyboard shortcuts for Alt+Up/Down (object reordering)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Only respond if Alt key is pressed and this panel has focus
                if !this.focus_handle.is_focused(window) || !event.keystroke.modifiers.alt {
                    return;
                }

                match event.keystroke.key.as_ref() {
                    "up" => {
                        // Move selected object up in hierarchy
                        if let Some(id) = this.state.selected_object() {
                            this.state.scene_database.move_object_up(&id);
                            cx.notify();
                        }
                    },
                    "down" => {
                        // Move selected object down in hierarchy
                        if let Some(id) = this.state.selected_object() {
                            this.state.scene_database.move_object_down(&id);
                            cx.notify();
                        }
                    },
                    _ => {}
                }
            }))
            .child(
                // Toolbar at the top
                self.toolbar.render(&self.state, self.shared_state.clone(), cx)
            )
            .child(
                // Workspace with draggable panels
                if let Some(ref workspace) = self.workspace {
                    workspace.clone().into_any_element()
                } else {
                    div().child("Loading workspace...").into_any_element()
                }
            )
            .child(
                // Status bar at the bottom
                self.render_status_bar(cx)
            )
    }
}

