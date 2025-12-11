use gpui::*;
use ui::{
    dock::{Panel, PanelEvent, DockItem, DockPlacement, DockChannel},
    workspace::Workspace,
    resizable::{h_resizable, v_resizable, resizable_panel, ResizableState},
    v_flex, h_flex,
    ActiveTheme as _, StyledExt,
};
// Zero-copy Bevy viewport for 3D rendering
use ui::bevy_viewport::{BevyViewport, BevyViewportState};

use ui::settings::EngineSettings;
// DEPRECATED: Old software renderers - replaced by Bevy
// use ui::ui::rainbow_engine_final::{RainbowRenderEngine, RainbowPattern};
// use ui::ui::wgpu_3d_renderer::Wgpu3DRenderer;
use engine_backend::services::gpu_renderer::GpuRenderer;
use ui_common::StatusBar;
use engine_backend::GameThread;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::rc::Rc;
use std::cell::RefCell;

use super::{
    LevelEditorState, SceneBrowser, HierarchyPanel, PropertiesPanel,
    ViewportPanel, ToolbarPanel, CameraMode, TransformTool,
};
use super::actions::*;
use crate::tabs::level_editor::scene_database::{
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
    scene_browser: SceneBrowser,
    hierarchy: HierarchyPanel,
    properties: PropertiesPanel,
    viewport_panel: ViewportPanel,
    toolbar: ToolbarPanel,

    // Layout state
    horizontal_resizable_state: Entity<ResizableState>,
    vertical_resizable_state: Entity<ResizableState>,

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
        let horizontal_resizable_state = ResizableState::new(cx);
        let vertical_resizable_state = ResizableState::new(cx);

        // Load engine settings for frame pacing configuration
        let settings = EngineSettings::default_path()
            .and_then(|path| Some(EngineSettings::load(&path)))
            .unwrap_or_default();

        let max_viewport_fps = settings.advanced.max_viewport_fps;

        // Create Bevy viewport with zero-copy shared textures
        let viewport = cx.new(|cx| BevyViewport::new(1600, 900, cx));
        let viewport_state = viewport.read(cx).shared_state();
        
        // Create game thread but DON'T start it yet (editor starts in Edit mode)
        let game_thread = Arc::new(GameThread::new(240.0));
        let game_state = game_thread.get_state();
        game_thread.set_enabled(false); // CRITICAL: Start disabled for Edit mode
        game_thread.start(); // Thread runs but does nothing while disabled
        
        // Create GPU render engine WITHOUT game thread link initially (Edit mode)
        let gpu_engine = Arc::new(Mutex::new(GpuRenderer::new(1600, 900))); // No game state initially
        let render_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Store GPU renderer in global EngineState using a marker that the render loop will pick up
        // The render loop will associate it with the correct window when it first renders
        if let Some(engine_state) = engine_state::EngineState::global() {
            if let Some(wid) = window_id {
                // We have the actual window ID - register directly!
                engine_state.set_window_gpu_renderer(wid, gpu_engine.clone());
            } else {
                // Fallback: Use a sentinel value (0) to mark this renderer as pending association with a window
                // The main render loop will detect windows with viewports and claim this renderer
                engine_state.set_window_gpu_renderer(0, gpu_engine.clone());
                engine_state.set_metadata("has_pending_viewport_renderer".to_string(), "true".to_string());
            }
        } else {
            println!("[LEVEL-EDITOR] ❌ ERROR: No global EngineState found!");
        }

        // Viewport stays transparent - Bevy renders directly to winit back buffer BEHIND GPUI
        // No texture initialization needed in GPUI - all handled in main.rs rendering loop
        println!("[LEVEL-EDITOR] 📺 Viewport configured as transparent (Bevy renders to back buffer)");

        
        println!("[LEVEL-EDITOR] Modular level editor initialized");

        let state = LevelEditorState::new();
        
        Self {
            focus_handle: cx.focus_handle(),
            state: LevelEditorState::new(), // Will be moved to shared_state
            fps_graph_is_line: Rc::new(RefCell::new(true)),  // Default to line graph
            scene_browser: SceneBrowser::new(),
            hierarchy: HierarchyPanel::new(),
            properties: PropertiesPanel::new(),
            viewport_panel: ViewportPanel::new(viewport.clone(), render_enabled.clone(), window, cx),
            toolbar: ToolbarPanel::new(),
            horizontal_resizable_state,
            vertical_resizable_state,
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
            // Use channel 3 for level editor to isolate from main app dock (channel 0), BP editor (channel 1), and script editor (channel 2)
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
                use crate::tabs::level_editor::ViewportPanelWrapper;
                ViewportPanelWrapper::new(
                    viewport_panel_inner,
                    shared_state.clone(),
                    fps_graph.clone(),
                    gpu.clone(),
                    game.clone(),
                    cx,
                )
            });
            
            // Create left panels (as tabs)
            let scene_panel = cx.new(|cx| {
                use crate::tabs::level_editor::SceneBrowserPanel;
                SceneBrowserPanel::new(cx)
            });
            
            let hierarchy_panel = cx.new(|cx| {
                use crate::tabs::level_editor::HierarchyPanelWrapper;
                HierarchyPanelWrapper::new(shared_state.clone(), cx)
            });
            
            // Create right panels (as tabs)
            let properties_panel = cx.new(|cx| {
                use crate::tabs::level_editor::PropertiesPanelWrapper;
                PropertiesPanelWrapper::new(shared_state.clone(), cx)
            });
            
            // Initialize workspace with draggable tabs on both sides
            workspace.initialize(
                DockItem::panel(std::sync::Arc::new(viewport_panel)),
                Some(DockItem::tabs(
                    vec![
                        std::sync::Arc::new(scene_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                        std::sync::Arc::new(hierarchy_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                    ],
                    Some(0),
                    &dock_area,
                    window,
                    cx,
                )),
                Some(DockItem::tabs(
                    vec![
                        std::sync::Arc::new(properties_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                    ],
                    Some(0),
                    &dock_area,
                    window,
                    cx,
                )),
                None,
                window,
                cx,
            );
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
            .unwrap_or_else(|| "None".to_string());
        
        StatusBar::new()
            .add_left_item(format!("Objects: {}", objects_count))
            .add_left_item(format!("Selected: {}", selected_name))
            .add_right_item(format!("{:?}", self.state.camera_mode))
            .add_right_item(format!("Grid: {}", if self.state.show_grid { "On" } else { "Off" }))
            .add_right_item(format!("Tool: {:?}", self.state.current_tool))
            .render(cx)
    }

    // Helper method to sync GPUI gizmo state to Bevy's shared resource
    fn sync_gizmo_to_bevy(&mut self) {
        if let Ok(engine) = self.gpu_engine.try_lock() {
            if let Some(ref bevy_renderer) = engine.bevy_renderer {
                if let Ok(mut bevy_gizmo) = bevy_renderer.gizmo_state.try_lock() {
                    let gpui_gizmo = self.state.gizmo_state.read();

                    // Map GPUI GizmoType to Bevy GizmoType
                    use engine_backend::subsystems::render::bevy_renderer::BevyGizmoType;
                    use crate::tabs::level_editor::GizmoType;
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

    fn on_toggle_snapping(&mut self, _: &ToggleSnapping, _: &mut Window, cx: &mut Context<Self>) {
        // Toggle snapping in gizmo state
        let mut gizmo_state = self.state.gizmo_state.write();
        gizmo_state.toggle_snap();
        let enabled = gizmo_state.snap_enabled;
        let increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_toggle_local_space(&mut self, _: &ToggleLocalSpace, _: &mut Window, cx: &mut Context<Self>) {
        // Toggle local/world space in gizmo state
        let mut gizmo_state = self.state.gizmo_state.write();
        gizmo_state.toggle_space();
        let is_local = gizmo_state.local_space;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_increase_snap_increment(&mut self, _: &IncreaseSnapIncrement, _: &mut Window, cx: &mut Context<Self>) {
        let mut gizmo_state = self.state.gizmo_state.write();
        // Double the snap increment (0.25, 0.5, 1.0, 2.0, 4.0, etc.)
        gizmo_state.snap_increment = (gizmo_state.snap_increment * 2.0).min(10.0);
        let increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_decrease_snap_increment(&mut self, _: &DecreaseSnapIncrement, _: &mut Window, cx: &mut Context<Self>) {
        let mut gizmo_state = self.state.gizmo_state.write();
        // Halve the snap increment (10.0, 5.0, 2.5, 1.0, 0.5, 0.25, etc.)
        gizmo_state.snap_increment = (gizmo_state.snap_increment / 2.0).max(0.1);
        let increment = gizmo_state.snap_increment;
        drop(gizmo_state);

        cx.notify();
    }

    fn on_focus_selected(&mut self, _: &FocusSelected, window: &mut Window, cx: &mut Context<Self>) {
        // TODO: Frame selected object in viewport (move camera to focus on selection)
        if let Some(obj) = self.state.get_selected_object() {
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
        if let Ok(engine_guard) = self.gpu_engine.try_lock() {
            if let Some(ref bevy_renderer) = engine_guard.bevy_renderer {
                let read_idx = bevy_renderer.get_read_index();
                let mut state = self.viewport_state.write();
                state.set_active_buffer(read_idx);

                // NO MORE DIRECT BEVY ACCESS - Everything goes through SharedGizmoStateResource
                // The sync_gizmo_state_system in Bevy will handle the rest
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
            // Transform tools - KEYBOARD: Q/W/E/R
            .on_action(cx.listener(Self::on_select_tool))
            .on_action(cx.listener(Self::on_move_tool))
            .on_action(cx.listener(Self::on_rotate_tool))
            .on_action(cx.listener(Self::on_scale_tool))
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
            .child(
                // Toolbar at the top
                self.toolbar.render(&self.state, cx)
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

