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
use engine_fs::virtual_fs;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use ui::settings::EngineSettings;
use ui::{notification::Notification, ContextModal as _};
use ui_common::StatusBar;

use crate::level_editor::state::PieStartRequest;

use super::actions::*;
use super::{toolbar, ToolbarPanel, ViewportPanel};
use crate::ai_sessions;
use crate::level_editor::scene_database::{
    LevelEditorCameraState, LightType, MeshType, ObjectType, SceneObjectData, Transform,
};
use crate::level_editor::{request_thumbnail_capture, CameraMode, LevelEditorState, TransformTool};
use engine_backend::scene::SceneDb;
use engine_backend::subsystems::render::EditorCameraState;
use plugin_manager;

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

    // Shared state for all panels (single source of truth)
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,

    // Workspace for draggable panels
    workspace: Option<Entity<Workspace>>,

    // Handles to sub-panels so we can forward notifications after scene mutations.
    hierarchy_panel_entity: Option<Entity<crate::level_editor::HierarchyPanelWrapper>>,
    properties_panel_entity: Option<Entity<crate::level_editor::PropertiesPanelWrapper>>,

    // Last scene revision observed by this panel. Used to detect AI-driven changes
    // that happen outside normal GPUI action handlers.
    last_observed_scene_revision: u64,

    // Keeps the polling task alive for the lifetime of the panel.
    _scene_revision_poller: gpui::Task<()>,
}

impl LevelEditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel = Self::new_internal(None, window, cx);
        Self::spawn_level_load(cx);
        panel
    }

    pub fn new_with_window_id(window_id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel = Self::new_internal(Some(window_id), window, cx);
        Self::spawn_level_load(cx);
        panel
    }

    /// Defers `ensure_default_level_file` to run after the editor window has
    /// rendered its first frame.
    ///
    /// By the time the loading screen closes, the scene directory already exists
    /// and the `default.level` file is in the OS page cache (both pre-warmed by
    /// the loading-screen background thread).  Even so, the actual deserialization
    /// happens here on the GPUI main thread — deferring it means the window
    /// becomes visible first, avoiding the "frozen / locked up" appearance on
    /// Windows.
    fn spawn_level_load(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // Yield once so the event loop can paint the first frame and make
            // the editor window visible before we touch disk.
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;

            // Back on the GPUI main thread: load the level file and notify
            // the viewport / panels to re-render with the scene contents.
            cx.update(|cx| {
                this.update(cx, |panel, cx| {
                    panel.ensure_default_level_file();
                    panel.notify_sub_panels(cx);
                    cx.notify();
                });
            });
        })
        .detach();
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
            if let Err(e) = engine_fs::virtual_fs::create_dir_all(parent) {
                tracing::warn!("Could not create default level directory {:?}: {e}", parent);
                return;
            }
        }

        let scene_db = { self.shared_state.read().scene.database.clone() };

        if default_path.exists() {
            // File already on disk — load it into the shared scene db.
            scene_db.clear();
            match scene_db.load_from_file_with_editor_camera(&default_path) {
                Ok(editor_camera) => {
                    self.apply_editor_camera_state(editor_camera.as_ref());
                    let mut w = self.shared_state.write();
                    w.scene.current_scene = Some(default_path);
                    w.scene.has_unsaved_changes = false;
                    if let Some(path) = w.scene.current_scene.clone() {
                        ai_sessions::register_open_scene(&path, &self.shared_state);
                    }
                }
                Err(e) => {
                    tracing::warn!("Default level exists but could not be loaded: {e}");
                }
            }
        } else {
            // File does not exist — seed from the embedded default.level if available,
            // otherwise save the current in-memory (empty) scene to disk.
            let embedded = engine_state::EngineContext::global()
                .and_then(|ctx| ctx.store.get_or_init::<Option<Vec<u8>>>().read().clone());

            let seed_result = if let Some(bytes) = embedded {
                // Write the embedded bytes directly — preserves whatever the developer
                // designed as the default scene via "Save as Default Level".
                engine_fs::virtual_fs::write_file(&default_path, &bytes)
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
                    let mut w = self.shared_state.write();
                    w.scene.current_scene = Some(default_path);
                    w.scene.has_unsaved_changes = false;
                    if let Some(path) = w.scene.current_scene.clone() {
                        ai_sessions::register_open_scene(&path, &self.shared_state);
                    }
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
        let scene_db = { panel.shared_state.read().scene.database.clone() };
        scene_db.clear();
        let editor_camera = scene_db.load_from_file_with_editor_camera(&path)?;
        panel.apply_editor_camera_state(editor_camera.as_ref());
        {
            let mut state = panel.shared_state.write();
            state.scene.current_scene = Some(path);
            state.scene.has_unsaved_changes = false;
            if let Some(open_path) = state.scene.current_scene.clone() {
                ai_sessions::register_open_scene(&open_path, &panel.shared_state);
            }
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

        // Get the physics query service from the global EngineBackend
        let physics_query = engine_backend::EngineBackend::global()
            .and_then(|backend| backend.read().get_physics_query_service());

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
        // The renderer and all panels now read/write the same Arc<SceneDb>.
        let mut state = LevelEditorState::new_with_scene_db(scene_db);

        // Attach the GPU renderer to SceneDatabase so every add/remove/update
        // immediately writes to BOTH SceneDb AND Helio (unified write path).

        let shared_state = Arc::new(parking_lot::RwLock::new(state));

        // Temporary debug toggle: replace viewport with a solid yellow panel to
        // verify layout/overlap issues independently of GPU rendering.
        let debug_replace_with_yellow = false;

        // Create HelioViewport — renders via WgpuSurfaceHandle every GPUI frame.
        // It receives shared_state so viewport drop actions mutate SceneDatabase
        // through the same command path as the rest of the editor.
        let viewport = cx.new(|cx| {
            HelioViewport::new(
                gpu_engine.clone(),
                shared_state.clone(),
                debug_replace_with_yellow,
                cx,
            )
        });

        // Poll for AI-driven scene mutations at 50 ms intervals.  AI tools
        // run on background threads and can't call cx.notify() directly, so
        // we bridge the gap here: whenever scene_revision advances we trigger
        // a GPUI re-render that propagates to the hierarchy and properties panels.
        let poll_state = Arc::clone(&shared_state);
        let poller = cx.spawn(async move |this, cx| {
            let mut last_seen: u64 = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let current = poll_state.read().scene.revision;
                if current != last_seen {
                    last_seen = current;
                    cx.update(|cx| {
                        this.update(cx, |panel, cx| {
                            panel.notify_sub_panels(cx);
                            cx.notify();
                        });
                    });
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
            shared_state,
            workspace: None,
            hierarchy_panel_entity: None,
            properties_panel_entity: None,
            last_observed_scene_revision: 0,
            _scene_revision_poller: poller,
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
                        shared_state.clone(),
                        fps_graph.clone(),
                        gpu.clone(),
                        cx,
                    )
                });

                // Create right dock panels
                let hierarchy_panel = cx.new(|cx| {
                    use crate::level_editor::HierarchyPanelWrapper;
                    HierarchyPanelWrapper::new(shared_state.clone(), window, cx)
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
        let state = self.shared_state.read();
        let objects_count = state.scene.scene_objects().len();
        let selected_name = state
            .scene
            .selected_object()
            .and_then(|id| state.scene.database.get_object(&id))
            .map(|obj| obj.name.clone())
            .unwrap_or_else(|| t!("LevelEditor.StatusBar.None").to_string());

        let grid_status = if state.editor.show_grid {
            t!("LevelEditor.StatusBar.GridOn").to_string()
        } else {
            t!("LevelEditor.StatusBar.GridOff").to_string()
        };

        let camera_mode_str = match state.editor.camera_mode {
            CameraMode::Perspective => t!("LevelEditor.CameraMode.Perspective").to_string(),
            CameraMode::Orthographic => t!("LevelEditor.CameraMode.Orthographic").to_string(),
            CameraMode::Top => t!("LevelEditor.CameraMode.Top").to_string(),
            CameraMode::Front => t!("LevelEditor.CameraMode.Front").to_string(),
            CameraMode::Side => t!("LevelEditor.CameraMode.Side").to_string(),
        };

        let tool_name = match state.editor.current_tool {
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

    fn sync_gizmo_to_helio(&mut self) {
        let tool = self.shared_state.read().editor.current_tool;
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
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Select);
        self.queue_gizmo_mode_for_tool(TransformTool::Select);
        cx.notify();
    }

    fn on_move_tool(&mut self, _: &MoveTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Move);
        self.queue_gizmo_mode_for_tool(TransformTool::Move);
        cx.notify();
    }

    fn on_rotate_tool(&mut self, _: &RotateTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Rotate);
        self.queue_gizmo_mode_for_tool(TransformTool::Rotate);
        cx.notify();
    }

    fn on_scale_tool(&mut self, _: &ScaleTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Scale);
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
        self.shared_state.write().play.time_scale = action.0;
        cx.notify();
    }

    fn on_set_multiplayer_mode(
        &mut self,
        action: &toolbar::SetMultiplayerMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().play.multiplayer_mode = action.0;
        cx.notify();
    }

    fn on_set_build_config(
        &mut self,
        action: &toolbar::SetBuildConfig,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.config = action.0;
        cx.notify();
    }

    fn on_set_target_platform(
        &mut self,
        action: &toolbar::SetTargetPlatform,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.target_platform = action.0;
        cx.notify();
    }

    fn on_set_build_mode(
        &mut self,
        action: &toolbar::SetBuildMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.mode = action.0;
        cx.notify();
    }

    fn on_add_object(&mut self, _: &AddObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let mut state = self.shared_state.write();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
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
                    component_instances: None,
                },
                parent_id: None,
            },
        );
        drop(state);
        self.notify_sub_panels(cx);
        cx.notify();
    }

    fn on_add_object_of_type(
        &mut self,
        action: &AddObjectOfType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let mut state = self.shared_state.write();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: format!("New {}", action.object_type),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
            },
        );
        drop(state);
        self.notify_sub_panels(cx);
        cx.notify();
    }

    fn on_delete_object(&mut self, _: &DeleteObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let selected = self.shared_state.read().scene.selected_object();
        if let Some(id) = selected {
            let mut state = self.shared_state.write();
            execute_command(&mut state, SceneCommand::RemoveObject { id });
            drop(state);
            self.sync_gizmo_to_helio();
        }
        self.notify_sub_panels(cx);
        cx.notify();
    }

    fn on_duplicate_object(&mut self, _: &DuplicateObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let selected = self.shared_state.read().scene.selected_object();
        if let Some(id) = selected {
            let mut state = self.shared_state.write();
            execute_command(
                &mut state,
                SceneCommand::DuplicateObject {
                    source_id: id,
                    count: 1,
                    position_offset: None,
                },
            );
        }
        self.notify_sub_panels(cx);
        cx.notify();
    }

    fn on_select_object(&mut self, action: &SelectObject, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .scene
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
            .hierarchy
            .toggle_object_expanded(&action.object_id);
        cx.notify();
    }

    fn on_toggle_grid(&mut self, _: &ToggleGrid, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_grid();
        cx.notify();
    }

    fn on_toggle_wireframe(&mut self, _: &ToggleWireframe, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_wireframe();
        cx.notify();
    }

    fn on_toggle_lighting(&mut self, _: &ToggleLighting, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_lighting();
        cx.notify();
    }

    fn on_toggle_performance_overlay(
        &mut self,
        _: &TogglePerformanceOverlay,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_performance_overlay();
        cx.notify();
    }

    fn on_toggle_camera_mode_selector(
        &mut self,
        _: &ToggleCameraModeSelector,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_camera_mode_selector();
        cx.notify();
    }

    fn on_toggle_viewport_options(
        &mut self,
        _: &ToggleViewportOptions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_viewport_options();
        cx.notify();
    }

    fn on_toggle_fps_graph_type(
        &mut self,
        _: &ToggleFpsGraphType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_fps_graph_type();
        cx.notify();
    }

    // Performance metrics toggles
    fn on_toggle_fps_graph(&mut self, _: &ToggleFpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().overlays.toggle_fps_graph();
        cx.notify();
    }

    fn on_toggle_tps_graph(&mut self, _: &ToggleTpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().overlays.toggle_tps_graph();
        cx.notify();
    }

    fn on_toggle_frame_time_graph(
        &mut self,
        _: &ToggleFrameTimeGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_frame_time_graph();
        cx.notify();
    }

    fn on_toggle_memory_graph(
        &mut self,
        _: &ToggleMemoryGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_memory_graph();
        cx.notify();
    }

    fn on_toggle_draw_calls_graph(
        &mut self,
        _: &ToggleDrawCallsGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_draw_calls_graph();
        cx.notify();
    }

    fn on_toggle_vertices_graph(
        &mut self,
        _: &ToggleVerticesGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_vertices_graph();
        cx.notify();
    }

    fn on_toggle_input_latency_graph(
        &mut self,
        _: &ToggleInputLatencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_input_latency_graph();
        cx.notify();
    }

    fn on_toggle_ui_consistency_graph(
        &mut self,
        _: &ToggleUiConsistencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_ui_consistency_graph();
        cx.notify();
    }

    fn on_play_scene(&mut self, _: &PlayScene, window: &mut Window, cx: &mut Context<Self>) {
        // Enter play mode (saves scene snapshot)
        self.shared_state.write().scene.enter_play_mode();

        // Disable gizmos in play mode
        self.sync_gizmo_to_helio();

        // Play In Editor (issue #243): build the project as a cdylib and hand it
        // to the viewport to embed. Without an open project we fall back to plain
        // play mode (snapshot only, no running game).
        match engine_state::get_project_path().map(std::path::PathBuf::from) {
            Some(root) => self.start_pie_build(root, window, cx),
            None => window.push_notification(
                Notification::warning("No project open — playing scene snapshot only."),
                cx,
            ),
        }

        cx.notify();
    }

    fn on_stop_scene(&mut self, _: &StopScene, _: &mut Window, cx: &mut Context<Self>) {
        // Ask the viewport to tear down the embedded game, then exit play mode.
        {
            let mut st = self.shared_state.write();
            st.play.pie.stop_requested = true;
            st.play.pie.pending_start = None;
            st.play.pie.building = false;
        }

        // Exit play mode (restores scene from snapshot)
        self.shared_state.write().scene.exit_play_mode();

        // Re-enable gizmos in edit mode
        self.sync_gizmo_to_helio();

        cx.notify();
    }

    /// Write the current scene to a temp `.level`, then build the project as a
    /// `cdylib` on a background thread. On success the viewport loads it.
    fn start_pie_build(
        &mut self,
        root: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Reflect unsaved edits: write the live SceneDb to a temp level file.
        let scene_path = root.join("target").join("pie").join("play.level");
        if let Err(e) = self
            .shared_state
            .read()
            .scene
            .database
            .save_to_file(&scene_path)
        {
            window.push_notification(
                Notification::error("Play In Editor").message(format!("Failed to write scene: {e}")),
                cx,
            );
            return;
        }

        {
            let mut st = self.shared_state.write();
            st.play.pie.building = true;
            st.play.pie.stop_requested = false;
            st.play.pie.last_error = None;
            st.play.pie.pending_start = None;
        }

        window.push_notification(
            Notification::info("Play In Editor").message("Building game…"),
            cx,
        );

        let shared = self.shared_state.clone();
        let _ = std::thread::Builder::new()
            .name("pie-build".into())
            .spawn(move || {
                let result = build_pie_dylib(&root, &scene_path);
                let mut st = shared.write();
                st.play.pie.building = false;
                match result {
                    Ok(req) => st.play.pie.pending_start = Some(req),
                    Err(e) => {
                        tracing::error!("PiE build failed: {e}");
                        st.play.pie.last_error = Some(e);
                    }
                }
            });
    }

    fn on_perspective_view(&mut self, _: &PerspectiveView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
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
            .editor
            .set_camera_mode(CameraMode::Orthographic);
        cx.notify();
    }

    fn on_top_view(&mut self, _: &TopView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Top);
        cx.notify();
    }

    fn on_front_view(&mut self, _: &FrontView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Front);
        cx.notify();
    }

    fn on_side_view(&mut self, _: &SideView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Side);
        cx.notify();
    }

    fn on_save_scene(&mut self, _: &SaveScene, _: &mut Window, cx: &mut Context<Self>) {
        // If no current scene path, do Save As
        if self.shared_state.read().scene.current_scene.is_none() {
            cx.dispatch_action(&SaveSceneAs);
            return;
        }

        let (scene_db, path_opt) = {
            let state = self.shared_state.read();
            (
                state.scene.database.clone(),
                state.scene.current_scene.clone(),
            )
        };

        if let Some(path) = path_opt {
            match scene_db
                .save_to_file_with_editor_camera(&path, self.current_editor_camera_state())
            {
                Ok(_) => {
                    self.shared_state.write().scene.has_unsaved_changes = false;
                    request_thumbnail_capture(&self.shared_state);
                    cx.notify();
                }
                Err(e) => {}
            }
        }
    }

    fn on_save_scene_as(&mut self, _: &SaveSceneAs, _window: &mut Window, cx: &mut Context<Self>) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene.database.clone() };
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
                                let previous = state_arc.write().scene.current_scene.clone();
                                if let Some(prev) = previous {
                                    ai_sessions::unregister_open_scene(&prev);
                                }
                                state_arc.write().scene.current_scene = Some(path);
                                state_arc.write().scene.has_unsaved_changes = false;
                                request_thumbnail_capture(&state_arc);
                                if let Some(open_path) =
                                    state_arc.read().scene.current_scene.clone()
                                {
                                    ai_sessions::register_open_scene(&open_path, &state_arc);
                                }
                            }
                            Err(e) => tracing::error!("Save failed: {}", e),
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn on_open_scene(&mut self, _: &OpenScene, _window: &mut Window, cx: &mut Context<Self>) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene.database.clone() };
        let default_dir = state_arc
            .read()
            .scene
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
                let result = scene_db.load_from_file_with_editor_camera(&path);
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        match result {
                            Ok(editor_camera) => {
                                this.apply_editor_camera_state(editor_camera.as_ref());
                                let mut state = state_arc.write();
                                if let Some(prev) = state.scene.current_scene.clone() {
                                    ai_sessions::unregister_open_scene(&prev);
                                }
                                state.scene.current_scene = Some(path);
                                state.scene.has_unsaved_changes = false;
                                // Deselect so properties panel clears stale data.
                                state.scene.select_object(None);
                                if let Some(open_path) = state.scene.current_scene.clone() {
                                    ai_sessions::register_open_scene(&open_path, &state_arc);
                                }
                            }
                            Err(e) => tracing::error!("Open scene failed: {}", e),
                        }
                        this.notify_sub_panels(cx);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn on_new_scene(&mut self, _: &NewScene, _: &mut Window, cx: &mut Context<Self>) {
        // Warn if unsaved changes (TODO: modal dialog)
        // Clear the scene IN-PLACE so the renderer keeps its Arc<SceneDb>.
        let scene_db = { self.shared_state.read().scene.database.clone() };
        let mut editor_camera = None;
        scene_db.clear();

        // Load from the embedded default.level if available, otherwise start empty.
        if let Some(bytes) = engine_state::EngineContext::global()
            .and_then(|ctx| ctx.store.get_or_init::<Option<Vec<u8>>>().read().clone())
        {
            let tmp = std::env::temp_dir().join("pulsar_new_scene_seed.level");
            if engine_fs::virtual_fs::write_file(&tmp, &bytes).is_ok() {
                match scene_db.load_from_file_with_editor_camera(&tmp) {
                    Ok(loaded_camera) => editor_camera = loaded_camera,
                    Err(e) => {
                        tracing::warn!("New scene: could not load embedded default.level: {e}")
                    }
                }
            }
        }
        self.apply_editor_camera_state(editor_camera.as_ref());
        self.notify_sub_panels(cx);
        {
            let mut state = self.shared_state.write();
            if let Some(prev) = state.scene.current_scene.clone() {
                ai_sessions::unregister_open_scene(&prev);
            }
            state.scene.current_scene = None;
            state.scene.has_unsaved_changes = false;
            // Deselect so properties panel clears stale data.
            state.scene.select_object(None);
        }
        cx.notify();
    }

    fn on_focus_selected(
        &mut self,
        _: &FocusSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // TODO: Frame selected object in viewport (move camera to focus on selection)
        if let Some(_obj) = self.shared_state.read().scene.get_selected_object() {
            // For now just log - implementing camera movement would require Bevy camera manipulation
        }
        cx.notify();
    }
}

impl Drop for LevelEditorPanel {
    fn drop(&mut self) {
        if let Some(path) = self.shared_state.read().scene.current_scene.clone() {
            ai_sessions::unregister_open_scene(&path);
        }
    }
}

impl Panel for LevelEditorPanel {
    fn panel_name(&self) -> &'static str {
        "Level Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let state = self.shared_state.read();
        div()
            .child(if let Some(ref scene) = state.scene.current_scene {
                format!(
                    "Level Editor - {}{}",
                    scene
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Untitled"),
                    if state.scene.has_unsaved_changes {
                        " *"
                    } else {
                        ""
                    }
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

    fn panel_file_path(&self, _cx: &App) -> Option<std::path::PathBuf> {
        self.shared_state.read().scene.current_scene.clone()
    }

    fn tab_icon(&self, _cx: &App) -> Option<ui::IconName> {
        let state = self.shared_state.read();
        let file_path = state.scene.current_scene.as_ref()?;

        // Get the file type icon from the plugin manager registry
        if let Some(plugin_mgr) = plugin_manager::global() {
            if let Some(file_type_def) = plugin_mgr.read().get_file_type_for_path(file_path) {
                return Some(file_type_def.icon.clone());
            }
        }

        None
    }

    fn tab_unsaved(&self, _cx: &App) -> bool {
        self.shared_state.read().scene.has_unsaved_changes
    }
}

ui_common::panel_boilerplate!(LevelEditorPanel);

impl EventEmitter<PanelEvent> for LevelEditorPanel {}

impl Render for LevelEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Initialize workspace on first render
        self.initialize_workspace(window, cx);

        // Apply external scene mutations (e.g. AI tool calls) to panel UI.
        let current_revision = self.shared_state.read().scene.revision;
        if current_revision != self.last_observed_scene_revision {
            self.last_observed_scene_revision = current_revision;
            self.notify_sub_panels(cx);
            cx.notify();
        }

        // Sync selection and gizmo state to Helio each frame via GpuRenderer API.
        if let Ok(mut engine_guard) = self.gpu_engine.try_lock() {
            let tool = self.shared_state.read().editor.current_tool;
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
            .on_action(cx.listener(Self::on_set_build_mode))
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
                        this.shared_state.write().scene.select_object(None);
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
                        if let Some(id) = this.shared_state.read().scene.selected_object() {
                            this.shared_state.read().scene.database.move_object_up(&id);
                            cx.notify();
                        }
                    }
                    "down" => {
                        // Move selected object down in hierarchy
                        if let Some(id) = this.shared_state.read().scene.selected_object() {
                            this.shared_state
                                .read()
                                .scene
                                .database
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

// ── Play In Editor build helpers (issue #243) ───────────────────────────────

/// Regenerate the project scaffolding and build it as a `cdylib`, returning what
/// the viewport needs to load the embedded game. Runs on a background thread.
fn build_pie_dylib(root: &Path, scene_path: &Path) -> Result<PieStartRequest, String> {
    // Ensure src/lib.rs + the `[lib] cdylib` manifest are up to date.
    engine_backend::services::ensure_core_bootstrap(root)?;

    let output = std::process::Command::new("cargo")
        .arg("build")
        .arg("--lib")
        .current_dir(root)
        .output()
        .map_err(|e| format!("Failed to spawn cargo: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Keep the message bounded — the full log is on stderr/tracing.
        let tail: String = stderr.lines().rev().take(20).collect::<Vec<_>>().join("\n");
        return Err(format!("cargo build --lib failed:\n{tail}"));
    }

    let crate_name = read_crate_name(root)?;
    let dylib_path = engine_backend::services::PieHost::output_dylib_path(root, &crate_name, false);
    if !dylib_path.exists() {
        return Err(format!(
            "Build succeeded but library not found at {}",
            dylib_path.display()
        ));
    }

    Ok(PieStartRequest {
        dylib_path,
        project_root: root.to_path_buf(),
        scene_path: scene_path.to_path_buf(),
    })
}

/// Read the `[package] name` from the project's `Cargo.toml`.
fn read_crate_name(root: &Path) -> Result<String, String> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("Failed to read Cargo.toml: {e}"))?;
    for line in toml.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                let name = value.trim().trim_matches('"').trim();
                if !name.is_empty() {
                    return Ok(name.to_string());
                }
            }
        }
    }
    Err("Could not find package name in Cargo.toml".to_string())
}
