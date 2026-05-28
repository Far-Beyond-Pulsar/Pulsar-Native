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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use ui::settings::EngineSettings;
use ui_common::StatusBar;

use super::actions::*;
use super::{toolbar, CameraMode, LevelEditorState, ToolbarPanel, TransformTool, ViewportPanel};
use super::state::StateEntity;
use crate::ai_sessions;
use crate::level_editor::scene_database::{
    LevelEditorCameraState, LightType, MeshType, ObjectType, SceneObjectData, Transform,
};
use engine_backend::scene::SceneDb;
use engine_backend::subsystems::render::EditorCameraState;
use plugin_manager;

/// Main Level Editor Panel - Orchestrates all sub-components
pub struct LevelEditorPanel {
    focus_handle: FocusHandle,

    fps_graph_is_line: Rc<RefCell<bool>>,
    toolbar: ToolbarPanel,

    viewport: Entity<HelioViewport>,
    gpu_engine: Arc<Mutex<GpuRenderer>>,
    render_enabled: Arc<AtomicBool>,
    game_thread: Arc<GameThread>,

    /// Lockless shared state — all panels observe this entity.
    /// Reads: `state.read(cx).field`  (zero-cost, no lock)
    /// Writes: `state.update(cx, |s, cx| { s.field = x; cx.notify(); })`
    state: StateEntity,

    /// Monotonic counter written by AI tools (off-GPUI-thread).
    /// A background task polls this and pushes increments into `state.scene_revision`.
    ai_revision: Arc<AtomicU64>,
    /// Whether any AI tool has made unsaved changes since last save.
    ai_has_unsaved: Arc<AtomicBool>,

    workspace: Option<Entity<Workspace>>,
    hierarchy_panel_entity: Option<Entity<crate::level_editor::HierarchyPanelWrapper>>,
    properties_panel_entity: Option<Entity<crate::level_editor::PropertiesPanelWrapper>>,

    /// Keeps the AI-revision watcher task alive.
    _ai_revision_watcher: gpui::Task<()>,
}

impl LevelEditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self::new_internal(None, window, cx);
        panel.ensure_default_level_file(cx);
        panel
    }

    pub fn new_with_window_id(window_id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self::new_internal(Some(window_id), window, cx);
        panel.ensure_default_level_file(cx);
        panel
    }

    /// If no project is open, do nothing. Otherwise resolve `<project>/scene/default.level`:
    /// - If the file already exists, load it.
    /// - If it doesn't exist, save the current in-memory default scene to it and
    ///   set `current_scene` so the title bar and save-as shortcuts work correctly.
    fn ensure_default_level_file(&mut self, cx: &mut Context<Self>) {
        let Some(project_str) = engine_state::get_project_path() else {
            return;
        };
        let default_path = std::path::PathBuf::from(&project_str)
            .join("scene")
            .join("default.level");

        if let Some(parent) = default_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("Could not create default level directory {:?}: {e}", parent);
                return;
            }
        }

        let scene_db = { self.state.read(cx).scene_database.clone() };

        if default_path.exists() {
            // File already on disk — load it into the shared scene db.
            scene_db.clear();
            match scene_db.load_from_file_with_editor_camera(&default_path) {
                Ok(editor_camera) => {
                    self.apply_editor_camera_state(editor_camera.as_ref());
                    // NOTE: ensure_default_level_file needs cx for entity writes.
                    // Deferred via cx.notify() after construction.
                }
                Err(e) => {
                    tracing::warn!("Default level exists but could not be loaded: {e}");
                }
            }
        } else {
            // File does not exist — seed from the embedded default.level if available,
            // otherwise save the current in-memory (empty) scene to disk.
            let embedded = engine_state::EngineContext::global()
                .and_then(|ctx| ctx.default_level_bytes.read().clone());

            let seed_result = if let Some(bytes) = embedded {
                // Write the embedded bytes directly — preserves whatever the developer
                // designed as the default scene via "Save as Default Level".
                std::fs::write(&default_path, &bytes)
                    .map_err(|e| format!("Failed to write embedded default level: {e}"))
            } else {
                // No embedded asset yet — persist the current empty scene so the
                // path is stable for future saves.
                scene_db.save_to_file_with_editor_camera(
                    &default_path,
                    self.current_editor_camera_state(),
                )
            };

            match seed_result {
                Ok(_) => {
                    // Load back what we just wrote so the editor shows the correct scene.
                    scene_db.clear();
                    match scene_db.load_from_file_with_editor_camera(&default_path) {
                        Ok(editor_camera) => {
                            self.apply_editor_camera_state(editor_camera.as_ref());
                            tracing::info!("Default level seeded at {:?}", default_path)
                        }
                        Err(e) => tracing::warn!("Seeded default level but reload failed: {e}"),
                    }
                    // NOTE: ensure_default_level_file needs cx for entity writes.
                    // Deferred via cx.notify() after construction.
                }
                Err(e) => {
                    tracing::warn!("Could not create default level at {:?}: {e}", default_path);
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
        let scene_db = { panel.state.read(cx).scene_database.clone() };
        scene_db.clear();
        let editor_camera = scene_db.load_from_file_with_editor_camera(&path)?;
        panel.apply_editor_camera_state(editor_camera.as_ref());
        panel.state.update(cx, |s, cx| {
            s.current_scene = Some(path.clone());
            s.has_unsaved_changes = false;
            cx.notify();
        });
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
        let state_value = LevelEditorState::new_with_scene_db(scene_db);

        // Wrap in a GPUI entity — lockless access from all panels on the UI thread.
        let state: StateEntity = cx.new(|_| state_value);

        // AI tools run off-thread; they write to SceneDatabase directly and
        // signal via these atomics.  A background watcher task propagates the
        // increment into state.scene_revision and calls cx.notify().
        let ai_revision:   Arc<AtomicU64>  = Arc::new(AtomicU64::new(0));
        let ai_has_unsaved: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        let debug_replace_with_yellow = false;

        // HelioViewport gets a clone of the state entity — same access pattern.
        let viewport = cx.new(|cx| {
            HelioViewport::new(
                gpu_engine.clone(),
                state.clone(),
                debug_replace_with_yellow,
                cx,
            )
        });

        // Lightweight watcher for AI-driven mutations.  Polls an atomic (cheap)
        // at 16 ms; on change pushes the new revision into the GPUI entity which
        // triggers reactive re-renders via cx.observe in each sub-panel.
        let rev_arc   = Arc::clone(&ai_revision);
        let unsaved_arc = Arc::clone(&ai_has_unsaved);
        let state_weak = state.downgrade();
        let watcher = cx.spawn(async move |_this, mut cx| {
            let mut last_seen: u64 = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                let current = rev_arc.load(Ordering::Relaxed);
                if current != last_seen {
                    last_seen = current;
                    let unsaved = unsaved_arc.swap(false, Ordering::Relaxed);
                    cx.update(|cx| {
                        if let Some(state) = state_weak.upgrade() {
                            state.update(cx, |s, cx| {
                                s.scene_revision = current;
                                if unsaved { s.has_unsaved_changes = true; }
                                cx.notify(); // reactive — all cx.observe() subscribers re-render
                            });
                        }
                    }).ok();
                }
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            fps_graph_is_line: Rc::new(RefCell::new(true)),
            toolbar: ToolbarPanel::new(),
            viewport,
            gpu_engine: gpu_engine.clone(),
            render_enabled,
            game_thread: game_thread.clone(),
            state,
            ai_revision,
            ai_has_unsaved,
            workspace: None,
            hierarchy_panel_entity: None,
            properties_panel_entity: None,
            _ai_revision_watcher: watcher,
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

        let state = self.state.clone();
        let fps_graph = self.fps_graph_is_line.clone();
        let gpu = self.gpu_engine.clone();
        let game = self.game_thread.clone();
        let viewport = self.viewport.clone();
        let render_enabled = self.render_enabled.clone();

        let (hierarchy_handle, properties_handle) =
            workspace.update(cx, |workspace, cx| {
                let dock_area = workspace.dock_area().downgrade();

                // Create viewport in center
                let viewport_panel_inner =
                    ViewportPanel::new(viewport.clone(), render_enabled.clone(), window, cx);
                let viewport_panel = cx.new(|cx| {
                    use crate::level_editor::ViewportPanelWrapper;
                    ViewportPanelWrapper::new(
                        viewport_panel_inner,
                        state.clone(),
                        fps_graph.clone(),
                        gpu.clone(),
                        game.clone(),
                        cx,
                    )
                });

                // Create right dock panels
                let hierarchy_panel = cx.new(|cx| {
                    use crate::level_editor::HierarchyPanelWrapper;
                    HierarchyPanelWrapper::new(state.clone(), window, cx)
                });
                let hierarchy_handle = hierarchy_panel.clone();
                let properties_panel = cx.new(|cx| {
                    use crate::level_editor::PropertiesPanelWrapper;
                    PropertiesPanelWrapper::new(state.clone(), window, cx)
                });
                let properties_handle = properties_panel.clone();
                let world_settings_panel = cx.new(|cx| {
                    use crate::level_editor::WorldSettingsPanel;
                    WorldSettingsPanel::new(state.clone(), window, cx)
                });

                // Wire up cross-panel notification: whenever the hierarchy is notified (e.g.
                // after a selection click), the properties panel is also notified so it
                // re-reads the selected object and updates its sections.
                {
                    let hierarchy_for_observe = hierarchy_panel.clone();
                    properties_panel.update(cx, |_, cx| {
                        cx.observe(&hierarchy_for_observe, |_, _, cx| {
                            cx.notify();
                        })
                        .detach();
                    });
                }

                // Bottom right: tabs for Properties and World Settings
                let bottom_tabs = DockItem::tabs(
                    vec![
                        std::sync::Arc::new(properties_panel)
                            as std::sync::Arc<dyn ui::dock::PanelView>,
                        std::sync::Arc::new(world_settings_panel)
                            as std::sync::Arc<dyn ui::dock::PanelView>,
                    ],
                    Some(0),
                    &dock_area,
                    window,
                    cx,
                );

                // Top right: hierarchy panel (as a single-tab TabPanel)
                let top_hierarchy = DockItem::tabs(
                    vec![std::sync::Arc::new(hierarchy_panel)
                        as std::sync::Arc<dyn ui::dock::PanelView>],
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
                    vec![Some(px(150.0)), Some(px(550.0))], // 150px hierarchy, 550px for Properties/World
                    &dock_area,
                    window,
                    cx,
                );

                // Set center and right dock only (no left dock, matching DAW approach)
                let center_tabs = DockItem::tabs(
                    vec![std::sync::Arc::new(viewport_panel)
                        as std::sync::Arc<dyn ui::dock::PanelView>],
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
        let state = self.state.read(cx);
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

    fn tool_to_gizmo(
        tool: TransformTool,
    ) -> (engine_backend::scene::GizmoType, engine_backend::GizmoMode) {
        use engine_backend::scene::GizmoType as SceneGizmoType;
        use engine_backend::GizmoMode;
        match tool {
            TransformTool::Select => (SceneGizmoType::None, GizmoMode::Translate),
            TransformTool::Move => (SceneGizmoType::Translate, GizmoMode::Translate),
            TransformTool::Rotate => (SceneGizmoType::Rotate, GizmoMode::Rotate),
            TransformTool::Scale => (SceneGizmoType::Scale, GizmoMode::Scale),
        }
    }

    fn sync_gizmo_to_helio_with_tool(&mut self, tool: TransformTool) {
        let (scene_type, helio_mode) = Self::tool_to_gizmo(tool);
        if let Ok(mut engine) = self.gpu_engine.lock() {
            engine.set_scene_gizmo_type(scene_type);
            engine.queue_gizmo_mode(helio_mode);
        }
    }

    fn queue_gizmo_mode_for_tool(&mut self, tool: TransformTool) {
        let (scene_type, helio_mode) = Self::tool_to_gizmo(tool);
        if let Ok(mut engine) = self.gpu_engine.lock() {
            engine.set_scene_gizmo_type(scene_type);
            engine.queue_gizmo_mode(helio_mode);
        }
    }

    fn current_editor_camera_state(&self) -> Option<LevelEditorCameraState> {
        self.gpu_engine
            .lock()
            .ok()
            .and_then(|engine| engine.editor_camera_state())
            .map(|camera| LevelEditorCameraState {
                position: camera.position,
                yaw: camera.yaw,
                pitch: camera.pitch,
            })
    }

    fn apply_editor_camera_state(&mut self, camera: Option<&LevelEditorCameraState>) {
        let Some(camera) = camera else {
            return;
        };

        if let Ok(mut engine) = self.gpu_engine.lock() {
            engine.set_editor_camera_state(EditorCameraState {
                position: camera.position,
                yaw: camera.yaw,
                pitch: camera.pitch,
            });
        }
    }

    // Action handlers
    fn on_select_tool(&mut self, _: &SelectTool, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.set_tool(TransformTool::Select); cx.notify(); });
        self.queue_gizmo_mode_for_tool(TransformTool::Select);
        cx.notify();
    }

    fn on_move_tool(&mut self, _: &MoveTool, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.set_tool(TransformTool::Move); cx.notify(); });
        self.queue_gizmo_mode_for_tool(TransformTool::Move);
        cx.notify();
    }

    fn on_rotate_tool(&mut self, _: &RotateTool, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.set_tool(TransformTool::Rotate); cx.notify(); });
        self.queue_gizmo_mode_for_tool(TransformTool::Rotate);
        cx.notify();
    }

    fn on_scale_tool(&mut self, _: &ScaleTool, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.set_tool(TransformTool::Scale); cx.notify(); });
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
        self.state.update(cx, |s, cx| { s.game_time_scale = action.0; cx.notify(); });
        cx.notify();
    }

    fn on_set_multiplayer_mode(
        &mut self,
        action: &toolbar::SetMultiplayerMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.multiplayer_mode = action.0; cx.notify(); });
        cx.notify();
    }

    fn on_set_build_config(
        &mut self,
        action: &toolbar::SetBuildConfig,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.build_config = action.0; cx.notify(); });
        cx.notify();
    }

    fn on_set_target_platform(
        &mut self,
        action: &toolbar::SetTargetPlatform,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.target_platform = action.0; cx.notify(); });
        cx.notify();
    }

    fn on_add_object(&mut self, _: &AddObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        self.state.update(cx, |state, cx| {
            execute_command(state, SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: "New Object".to_string(),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                },
                parent_id: None,
            });
            cx.notify();
        });
                cx.notify();
    }

    fn on_add_object_of_type(
        &mut self,
        action: &AddObjectOfType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let obj_type = action.object_type.clone();
        self.state.update(cx, |state, cx| {
            execute_command(state, SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: format!("New {obj_type}"),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                },
                parent_id: None,
            });
            cx.notify();
        });
                cx.notify();
    }

    fn on_delete_object(&mut self, _: &DeleteObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let selected = self.state.read(cx).selected_object();
        if let Some(id) = selected {
            self.state.update(cx, |state, cx| {
                execute_command(state, SceneCommand::RemoveObject { id: id.clone() });
                cx.notify();
            });
            { let tool = self.state.read(cx).current_tool; self.sync_gizmo_to_helio_with_tool(tool); }
        }
                cx.notify();
    }

    fn on_duplicate_object(&mut self, _: &DuplicateObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let selected = self.state.read(cx).selected_object();
        if let Some(id) = selected {
            self.state.update(cx, |state, cx| {
                execute_command(state, SceneCommand::DuplicateObject {
                    source_id: id.clone(), count: 1, position_offset: None,
                });
                cx.notify();
            });
        }
                cx.notify();
    }

    fn on_select_object(&mut self, action: &SelectObject, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.select_object(Some(action.object_id.clone())); cx.notify(); });
        { let tool = self.state.read(cx).current_tool; self.sync_gizmo_to_helio_with_tool(tool); } // Sync gizmo to follow selected object
                cx.notify();
    }

    fn on_toggle_object_expanded(
        &mut self,
        action: &ToggleObjectExpanded,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_object_expanded(&action.object_id); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_grid(&mut self, _: &ToggleGrid, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.toggle_grid(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_wireframe(&mut self, _: &ToggleWireframe, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.toggle_wireframe(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_lighting(&mut self, _: &ToggleLighting, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.toggle_lighting(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_performance_overlay(
        &mut self,
        _: &TogglePerformanceOverlay,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_performance_overlay(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_camera_mode_selector(
        &mut self,
        _: &ToggleCameraModeSelector,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_camera_mode_selector(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_viewport_options(
        &mut self,
        _: &ToggleViewportOptions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_viewport_options(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_fps_graph_type(
        &mut self,
        _: &ToggleFpsGraphType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_fps_graph_type(); cx.notify(); });
        cx.notify();
    }

    // Performance metrics toggles
    fn on_toggle_fps_graph(&mut self, _: &ToggleFpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.toggle_fps_graph(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_tps_graph(&mut self, _: &ToggleTpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.toggle_tps_graph(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_frame_time_graph(
        &mut self,
        _: &ToggleFrameTimeGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_frame_time_graph(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_memory_graph(
        &mut self,
        _: &ToggleMemoryGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_memory_graph(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_draw_calls_graph(
        &mut self,
        _: &ToggleDrawCallsGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_draw_calls_graph(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_vertices_graph(
        &mut self,
        _: &ToggleVerticesGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_vertices_graph(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_input_latency_graph(
        &mut self,
        _: &ToggleInputLatencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_input_latency_graph(); cx.notify(); });
        cx.notify();
    }

    fn on_toggle_ui_consistency_graph(
        &mut self,
        _: &ToggleUiConsistencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.toggle_ui_consistency_graph(); cx.notify(); });
        cx.notify();
    }

    fn on_play_scene(&mut self, _: &PlayScene, _: &mut Window, cx: &mut Context<Self>) {
        // Enter play mode (saves scene snapshot)
        self.state.update(cx, |s, cx| { s.enter_play_mode(); cx.notify(); });

        // Enable game thread
        self.game_thread.set_enabled(true);

        // Disable gizmos in play mode
        { let tool = self.state.read(cx).current_tool; self.sync_gizmo_to_helio_with_tool(tool); }

        cx.notify();
    }

    fn on_stop_scene(&mut self, _: &StopScene, _: &mut Window, cx: &mut Context<Self>) {
        // Disable game thread
        self.game_thread.set_enabled(false);

        // Exit play mode (restores scene from snapshot)
        self.state.update(cx, |s, cx| { s.exit_play_mode(); cx.notify(); });

        // Re-enable gizmos in edit mode
        { let tool = self.state.read(cx).current_tool; self.sync_gizmo_to_helio_with_tool(tool); }

        cx.notify();
    }

    fn on_perspective_view(&mut self, _: &PerspectiveView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.set_camera_mode(CameraMode::Perspective); cx.notify(); });
        cx.notify();
    }

    fn on_orthographic_view(
        &mut self,
        _: &OrthographicView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| { s.set_camera_mode(CameraMode::Orthographic); cx.notify(); });
        cx.notify();
    }

    fn on_top_view(&mut self, _: &TopView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.set_camera_mode(CameraMode::Top); cx.notify(); });
        cx.notify();
    }

    fn on_front_view(&mut self, _: &FrontView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.set_camera_mode(CameraMode::Front); cx.notify(); });
        cx.notify();
    }

    fn on_side_view(&mut self, _: &SideView, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| { s.set_camera_mode(CameraMode::Side); cx.notify(); });
        cx.notify();
    }

    fn on_save_scene(&mut self, _: &SaveScene, _: &mut Window, cx: &mut Context<Self>) {
        // If no current scene path, do Save As
        if self.state.read(cx).current_scene.is_none() {
            cx.dispatch_action(&SaveSceneAs);
            return;
        }

        let (scene_db, path_opt) = {
            let state = self.state.read(cx);
            (state.scene_database.clone(), state.current_scene.clone())
        };

        if let Some(path) = path_opt {
            match scene_db.save_to_file_with_editor_camera(&path, self.current_editor_camera_state()) {
                Ok(_) => {
                    self.state.update(cx, |s, cx| { s.has_unsaved_changes = false; cx.notify(); });
                    cx.notify();
                }
                Err(e) => {}
            }
        }
    }

    fn on_save_scene_as(&mut self, _: &SaveSceneAs, _window: &mut Window, cx: &mut Context<Self>) {
        let state_arc = self.state.clone();
        let scene_db = { self.state.read(cx).scene_database.clone() };
        let editor_camera = self.current_editor_camera_state();
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Save Scene As")
            .add_filter("Level file", &["level", "json"])
            .set_file_name("untitled.level");
        cx.spawn(async move |_this, cx| {
            if let Some(handle) = dialog.save_file().await {
                let path = handle.path().to_path_buf();
                let result = scene_db.save_to_file_with_editor_camera(&path, editor_camera);
                cx.update(|cx| {
                    _this.update(cx, |_, cx| {
                        match result {
                            Ok(_) => {
                                state_arc.update(cx, |state, cx| {
                                    if let Some(prev) = state.current_scene.take() {
                                        ai_sessions::unregister_open_scene(&prev);
                                    }
                                    state.current_scene = Some(path.clone());
                                    state.has_unsaved_changes = false;
                                    cx.notify();
                                });
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
        let state_arc = self.state.clone();
        let (scene_db, default_dir) = {
            let s = self.state.read(cx);
            (s.scene_database.clone(), s.current_scene.as_ref()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()))
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Open Scene")
            .add_filter("Level file", &["level", "json"])
            .set_directory(default_dir);
        cx.spawn(async move |this, cx| {
            if let Some(handle) = dialog.pick_file().await {
                let path = handle.path().to_path_buf();
                // Load into the existing shared SceneDb (renderer keeps its Arc).
                let result = scene_db.load_from_file_with_editor_camera(&path);
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        match result {
                            Ok(editor_camera) => {
                                this.apply_editor_camera_state(editor_camera.as_ref());
                                state_arc.update(cx, |state, cx| {
                                    if let Some(prev) = state.current_scene.take() {
                                        ai_sessions::unregister_open_scene(&prev);
                                    }
                                    state.current_scene = Some(path.clone());
                                    state.has_unsaved_changes = false;
                                    state.select_object(None);
                                    cx.notify();
                                });
                            }
                            Err(e) => tracing::error!("Open scene failed: {}", e),
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

    fn on_new_scene(&mut self, _: &NewScene, _: &mut Window, cx: &mut Context<Self>) {
        // Warn if unsaved changes (TODO: modal dialog)
        // Clear the scene IN-PLACE so the renderer keeps its Arc<SceneDb>.
        let scene_db = { self.state.read(cx).scene_database.clone() };
        let mut editor_camera = None;
        scene_db.clear();

        // Load from the embedded default.level if available, otherwise start empty.
        if let Some(bytes) = engine_state::EngineContext::global()
            .and_then(|ctx| ctx.default_level_bytes.read().clone())
        {
            let tmp = std::env::temp_dir().join("pulsar_new_scene_seed.level");
            if std::fs::write(&tmp, &bytes).is_ok() {
                match scene_db.load_from_file_with_editor_camera(&tmp) {
                    Ok(loaded_camera) => editor_camera = loaded_camera,
                    Err(e) => tracing::warn!("New scene: could not load embedded default.level: {e}"),
                }
            }
        }
        self.apply_editor_camera_state(editor_camera.as_ref());
                self.state.update(cx, |state, cx| {
            if let Some(prev) = state.current_scene.take() {
                ai_sessions::unregister_open_scene(&prev);
            }
            state.has_unsaved_changes = false;
            state.select_object(None);
            cx.notify();
        });
        cx.notify();
    }

    fn on_focus_selected(
        &mut self,
        _: &FocusSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // TODO: Frame selected object in viewport (move camera to focus on selection)
        if let Some(_obj) = self.state.read(cx).get_selected_object() {
            // For now just log - implementing camera movement would require Bevy camera manipulation
        }
        cx.notify();
    }
}

// Drop notification handled by ai_sessions cleanup on scene close.

impl Panel for LevelEditorPanel {
    fn panel_name(&self) -> &'static str {
        "Level Editor"
    }

    fn title(&self, _window: &Window, cx: &App) -> AnyElement {
        let state = self.state.read(cx);
        div()
            .child(if let Some(ref scene) = state.current_scene {
                format!(
                    "Level Editor - {}{}",
                    scene
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Untitled"),
                    if state.has_unsaved_changes { " *" } else { "" }
                )
            } else {
                "Level Editor".to_string()
            })
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }

    fn panel_file_path(&self, cx: &App) -> Option<std::path::PathBuf> {
        self.state.read(cx).current_scene.clone()
    }

    fn tab_icon(&self, cx: &App) -> Option<ui::IconName> {
        let state = self.state.read(cx);
        let file_path = state.current_scene.as_ref()?;

        // Get the file type icon from the plugin manager registry
        if let Some(plugin_mgr) = plugin_manager::global() {
            if let Ok(plugin_mgr_guard) = plugin_mgr.read() {
                if let Some(file_type_def) = plugin_mgr_guard.get_file_type_for_path(file_path) {
                    return Some(file_type_def.icon.clone());
                }
            }
        }

        None
    }

    fn tab_unsaved(&self, cx: &App) -> bool {
        self.state.read(cx).has_unsaved_changes
    }
}

ui_common::panel_boilerplate!(LevelEditorPanel);

impl EventEmitter<PanelEvent> for LevelEditorPanel {}

impl Render for LevelEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Initialize workspace on first render
        self.initialize_workspace(window, cx);

        // scene_revision changes are propagated via cx.observe in sub-panel constructors.
        // No manual notification or polling needed here.

        // Sync selection and gizmo state to Helio each frame via GpuRenderer API.
        if let Ok(mut engine_guard) = self.gpu_engine.try_lock() {
            let tool = self.state.read(cx).current_tool;
            let (scene_type, _) = Self::tool_to_gizmo(tool);
            if scene_type != engine_guard.get_scene_gizmo_type() {
                engine_guard.set_scene_gizmo_type(scene_type);
            }
            engine_guard.sync_selection_to_helio();
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
                        this.state.update(cx, |s, cx| { s.select_object(None); cx.notify(); });
                        // Signal the render thread to call editor_state.deselect() next frame.
                        if let Ok(engine) = this.gpu_engine.lock() {
                            engine.queue_deselect();
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
                    "l" => {}
                    "f" => cx.dispatch_action(&FocusSelected),
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
                        if let Some(id) = this.state.read(cx).selected_object() {
                            this.state.read(cx).scene_database.move_object_up(&id);
                            cx.notify();
                        }
                    }
                    "down" => {
                        if let Some(id) = this.state.read(cx).selected_object() {
                            this.state.read(cx).scene_database.move_object_down(&id);
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .child(
                // Toolbar at the top
                {
                    let s = self.state.read(cx).clone();  // owned clone, releases borrow on cx
                    self.toolbar.render(&s, self.state.clone(), self.gpu_engine.clone(), cx)
                },
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
