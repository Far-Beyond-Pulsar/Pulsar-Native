use std::path::PathBuf;
use std::collections::HashSet;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// Import our scene database and gizmo systems
use crate::level_editor::{SceneDatabase, GizmoState, GizmoType};

/// Editor mode - Edit or Play
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Edit,  // Editing mode - gizmos active, game thread paused
    Play,  // Play mode - game running, gizmos hidden
}

/// Hierarchy drag state for reparenting objects
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HierarchyDragState {
    None,
    DraggingObject {
        object_id: String,
        original_parent: Option<String>,
    },
}

/// Shared state for the Level Editor
#[derive(Clone)]
pub struct LevelEditorState {
    /// Scene database - single source of truth for all scene data
    pub scene_database: SceneDatabase,
    /// Snapshot of scene state when entering play mode (for reset on stop)
    pub scene_snapshot: Option<Arc<parking_lot::RwLock<Vec<crate::level_editor::scene_database::SceneObjectData>>>>,
    /// Gizmo state for 3D manipulation
    pub gizmo_state: Arc<parking_lot::RwLock<GizmoState>>,
    /// Current editor mode
    pub editor_mode: EditorMode,
    /// Currently open scene file
    pub current_scene: Option<PathBuf>,
    /// Whether the scene has unsaved changes
    pub has_unsaved_changes: bool,
    /// Current transform tool (Select, Move, Rotate, Scale)
    pub current_tool: TransformTool,
    /// Viewport camera mode
    pub camera_mode: CameraMode,
    /// Viewport rendering options
    pub show_wireframe: bool,
    pub show_lighting: bool,
    pub show_grid: bool,
    pub show_performance_overlay: bool,
    pub show_gpu_pipeline_overlay: bool,
    pub show_camera_mode_selector: bool,
    pub show_viewport_options: bool,
    /// Collapsed state for overlays (when X is clicked, overlay collapses to a button)
    pub camera_mode_selector_collapsed: bool,
    pub viewport_options_collapsed: bool,
    pub performance_overlay_collapsed: bool,
    pub gpu_pipeline_overlay_collapsed: bool,
    /// FPS graph type (true = line, false = bar)
    pub fps_graph_is_line: bool,
    /// Performance metrics visibility
    pub show_fps_graph: bool,
    pub show_tps_graph: bool,
    pub show_frame_time_graph: bool,
    pub show_memory_graph: bool,
    pub show_draw_calls_graph: bool,
    pub show_vertices_graph: bool,
    pub show_input_latency_graph: bool,
    pub show_ui_consistency_graph: bool,
    /// Expanded state for hierarchy items
    pub expanded_objects: HashSet<String>,
    /// Drag state for hierarchy reparenting
    pub hierarchy_drag_state: HierarchyDragState,
    /// Overlay positions (in pixels from their default corners)
    pub camera_overlay_pos: (f32, f32),  // bottom-left overlay position
    pub viewport_overlay_pos: (f32, f32), // top-left overlay position
    /// Dragging state for overlays
    pub is_dragging_camera_overlay: bool,
    pub is_dragging_viewport_overlay: bool,
    pub camera_overlay_drag_start: Option<(f32, f32)>, // mouse position when drag started
    pub viewport_overlay_drag_start: Option<(f32, f32)>,
    /// Camera movement speed (shared between UI and input)
    pub camera_move_speed: f32,
    /// Game control toolbar state
    pub game_time_scale: f32,
    pub game_target_fps: u32,
    pub multiplayer_mode: MultiplayerMode,
    pub build_config: BuildConfig,
    pub target_platform: TargetPlatform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiplayerMode {
    Offline,
    Host,
    Client,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildConfig {
    Debug,
    Release,
    Shipping,
}

/// Complete Rust target platform and architecture support (excluding WASM)
/// Currently supports ~50 most common targets. Additional targets can be added as needed.
/// See `rustc --print target-list` for the full list of 290+ available targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetPlatform {
    // Windows targets
    WindowsX86_64Msvc,
    WindowsI686Msvc,
    WindowsAarch64Msvc,
    WindowsX86_64Gnu,
    WindowsI686Gnu,
    
    // Linux targets
    LinuxX86_64Gnu,
    LinuxI686Gnu,
    LinuxAarch64Gnu,
    LinuxArmv7Gnueabihf,
    LinuxArmGnueabi,
    LinuxArmGnueabihf,
    LinuxMips64Gnuabi64,
    LinuxMips64elGnuabi64,
    LinuxMipsGnu,
    LinuxMipselGnu,
    LinuxPowerpc64Gnu,
    LinuxPowerpc64leGnu,
    LinuxPowerpcGnu,
    LinuxRiscv64Gc,
    LinuxS390xGnu,
    LinuxSparcv9,
    LinuxX86_64Musl,
    LinuxAarch64Musl,
    LinuxArmv7Musleabihf,
    LinuxMipselMusl,
    LinuxMipsMusl,
    
    // macOS targets
    MacOsX86_64,
    MacOsAarch64,
    
    // iOS targets
    IosAarch64,
    IosX86_64,
    IosAarch64Sim,
    
    // Android targets
    AndroidAarch64,
    AndroidArmv7,
    AndroidI686,
    AndroidX86_64,
    
    // BSD targets
    FreeBsdX86_64,
    FreeBsdI686,
    NetBsdX86_64,
    OpenBsdX86_64,
    DragonFlyX86_64,
    
    // Solaris
    SolarisSparcv9,
    SolarisX86_64,
    IlumosX86_64,
    
    // Redox
    RedoxX86_64,
    
    // Fuchsia
    FuchsiaAarch64,
    FuchsiaX86_64,
    
    // Gaming Consoles - PlayStation
    PlayStationPs4,
    PlayStationPs5,
    
    // Gaming Consoles - Xbox
    XboxOne,
    XboxSeriesXS,
    
    // Gaming Consoles - Nintendo
    NintendoSwitch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformTool {
    Select,
    Move,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraMode {
    Perspective,
    Orthographic,
    Top,
    Front,
    Side,
}

// Legacy types for backwards compatibility - now forwarded from scene_database
pub use crate::level_editor::scene_database::{
    Transform,
    SceneObjectData as SceneObject,
};

impl Default for LevelEditorState {
    fn default() -> Self {
        // Create scene database with default objects matching Bevy renderer
        let scene_database = SceneDatabase::with_default_scene();
        
        // Create gizmo state with translate tool active
        let mut gizmo_state = GizmoState::new();
        gizmo_state.set_gizmo_type(GizmoType::Translate);
        
        Self {
            scene_database,
            scene_snapshot: None,
            gizmo_state: Arc::new(parking_lot::RwLock::new(gizmo_state)),
            editor_mode: EditorMode::Edit, // Start in edit mode
            current_scene: None,
            has_unsaved_changes: false,
            current_tool: TransformTool::Move,
            camera_mode: CameraMode::Perspective,
            show_wireframe: false,
            show_lighting: true,
            show_grid: true,
            show_performance_overlay: false,
            show_gpu_pipeline_overlay: false,
            show_camera_mode_selector: true,
            show_viewport_options: true,
            camera_mode_selector_collapsed: false,
            viewport_options_collapsed: false,
            performance_overlay_collapsed: false,
            gpu_pipeline_overlay_collapsed: false,
            fps_graph_is_line: true,
            // Performance metrics - default: Frame Time, GPU Memory, Input Latency
            show_fps_graph: false,
            show_tps_graph: false,
            show_frame_time_graph: true,
            show_memory_graph: true,
            show_draw_calls_graph: false,
            show_vertices_graph: false,
            show_input_latency_graph: true,
            show_ui_consistency_graph: false,
            expanded_objects: HashSet::new(),
            hierarchy_drag_state: HierarchyDragState::None,
            camera_overlay_pos: (16.0, 16.0),  // default bottom-left position
            viewport_overlay_pos: (16.0, 16.0), // default top-left position
            is_dragging_camera_overlay: false,
            is_dragging_viewport_overlay: false,
            camera_overlay_drag_start: None,
            viewport_overlay_drag_start: None,
            camera_move_speed: 10.0,
            game_time_scale: 1.0,
            game_target_fps: 60,
            multiplayer_mode: MultiplayerMode::Offline,
            build_config: BuildConfig::Debug,
            target_platform: TargetPlatform::WindowsX86_64Msvc, // Default to Windows x64 MSVC
        }
    }
}

impl LevelEditorState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter play mode - snapshot scene and start game thread
    pub fn enter_play_mode(&mut self) {
        tracing::debug!("[EDITOR] 🎮 Entering PLAY mode");
        
        // Save snapshot of current scene state for restoration
        let objects = self.scene_database.get_all_objects();
        self.scene_snapshot = Some(Arc::new(parking_lot::RwLock::new(objects)));
        
        // Switch to play mode
        self.editor_mode = EditorMode::Play;
        
        // Hide gizmos
        let mut gizmo = self.gizmo_state.write();
        gizmo.set_gizmo_type(GizmoType::None);
        
        tracing::debug!("[EDITOR] ✅ Play mode active - game thread will start");
    }

    /// Exit play mode - restore scene state and stop game thread
    pub fn exit_play_mode(&mut self) {
        tracing::debug!("[EDITOR] 🛑 Exiting PLAY mode");
        
        // Restore scene from snapshot
        if let Some(ref snapshot) = self.scene_snapshot {
            let objects = snapshot.read().clone();
            
            // Clear current scene
            self.scene_database.clear();
            
            // Restore all objects
            for obj in objects {
                self.scene_database.add_object(obj, None);
            }
            
            tracing::debug!("[EDITOR] ✅ Scene restored from snapshot");
        }
        
        // Switch back to edit mode
        self.editor_mode = EditorMode::Edit;
        
        // Restore gizmo based on current tool
        let gizmo_type = match self.current_tool {
            TransformTool::Select => GizmoType::None,
            TransformTool::Move => GizmoType::Translate,
            TransformTool::Rotate => GizmoType::Rotate,
            TransformTool::Scale => GizmoType::Scale,
        };
        
        let mut gizmo = self.gizmo_state.write();
        gizmo.set_gizmo_type(gizmo_type);
        
        // Clear snapshot
        self.scene_snapshot = None;
        
        tracing::debug!("[EDITOR] ✅ Edit mode active");
    }

    /// Check if in edit mode
    pub fn is_edit_mode(&self) -> bool {
        self.editor_mode == EditorMode::Edit
    }

    /// Check if in play mode
    pub fn is_play_mode(&self) -> bool {
        self.editor_mode == EditorMode::Play
    }

    /// Get selected object ID
    pub fn selected_object(&self) -> Option<String> {
        self.scene_database.get_selected_object_id()
    }

    /// Get all scene objects for hierarchy display
    pub fn scene_objects(&self) -> Vec<SceneObject> {
        self.scene_database.get_root_objects()
    }

    /// Select an object
    pub fn select_object(&mut self, object_id: Option<String>) {
        self.scene_database.select_object(object_id.clone());

        // Update gizmo target
        let mut gizmo = self.gizmo_state.write();
        gizmo.target_object_id = object_id.clone();

        if let Some(ref id) = object_id {
            tracing::debug!("[STATE] 🎯 Selected object: '{}', gizmo will follow", id);
        } else {
            tracing::debug!("[STATE] 🚫 Deselected object, gizmo hidden");
        }
    }

    /// Get selected object data
    pub fn get_selected_object(&self) -> Option<SceneObject> {
        self.scene_database.get_selected_object()
    }

    /// Set the current transform tool (only in edit mode)
    pub fn set_tool(&mut self, tool: TransformTool) {
        if !self.is_edit_mode() {
            return; // Ignore tool changes in play mode
        }

        self.current_tool = tool;

        // Update gizmo type in the shared gizmo state
        let gizmo_type = match tool {
            TransformTool::Select => GizmoType::None,
            TransformTool::Move => GizmoType::Translate,
            TransformTool::Rotate => GizmoType::Rotate,
            TransformTool::Scale => GizmoType::Scale,
        };

        let mut gizmo = self.gizmo_state.write();
        gizmo.set_gizmo_type(gizmo_type);

        tracing::debug!("[STATE] 🎯 Tool changed to {:?}, gizmo type: {:?}", tool, gizmo_type);
    }

    /// Set camera mode
    pub fn set_camera_mode(&mut self, mode: CameraMode) {
        self.camera_mode = mode;
    }

    /// Toggle object expanded state in hierarchy
    pub fn toggle_object_expanded(&mut self, object_id: &str) {
        if self.expanded_objects.contains(object_id) {
            self.expanded_objects.remove(object_id);
        } else {
            self.expanded_objects.insert(object_id.to_string());
        }
    }

    /// Check if object is expanded in hierarchy
    pub fn is_object_expanded(&self, object_id: &str) -> bool {
        self.expanded_objects.contains(object_id)
    }

    /// Expand all objects in hierarchy
    pub fn expand_all(&mut self) {
        fn expand_recursive(objects: &[SceneObject], set: &mut HashSet<String>) {
            for obj in objects {
                if !obj.children.is_empty() {
                    set.insert(obj.id.clone());
                }
            }
        }
        expand_recursive(&self.scene_objects(), &mut self.expanded_objects);
    }

    /// Collapse all objects in hierarchy
    pub fn collapse_all(&mut self) {
        self.expanded_objects.clear();
    }

    /// Toggle grid visibility
    pub fn toggle_grid(&mut self) {
        self.show_grid = !self.show_grid;
    }

    /// Toggle wireframe rendering
    pub fn toggle_wireframe(&mut self) {
        self.show_wireframe = !self.show_wireframe;
    }

    /// Toggle lighting
    pub fn toggle_lighting(&mut self) {
        self.show_lighting = !self.show_lighting;
    }

    /// Toggle performance overlay
    pub fn toggle_performance_overlay(&mut self) {
        self.show_performance_overlay = !self.show_performance_overlay;
    }

    /// Toggle camera mode selector
    pub fn toggle_camera_mode_selector(&mut self) {
        self.show_camera_mode_selector = !self.show_camera_mode_selector;
    }

    /// Toggle viewport options
    pub fn toggle_viewport_options(&mut self) {
        self.show_viewport_options = !self.show_viewport_options;
    }

    /// Collapse/expand camera mode selector
    pub fn set_camera_mode_selector_collapsed(&mut self, collapsed: bool) {
        self.camera_mode_selector_collapsed = collapsed;
    }

    /// Collapse/expand viewport options
    pub fn set_viewport_options_collapsed(&mut self, collapsed: bool) {
        self.viewport_options_collapsed = collapsed;
    }

    /// Collapse/expand performance overlay
    pub fn set_performance_overlay_collapsed(&mut self, collapsed: bool) {
        self.performance_overlay_collapsed = collapsed;
    }

    /// Collapse/expand GPU pipeline overlay
    pub fn set_gpu_pipeline_overlay_collapsed(&mut self, collapsed: bool) {
        self.gpu_pipeline_overlay_collapsed = collapsed;
    }

    /// Toggle FPS graph type
    pub fn toggle_fps_graph_type(&mut self) {
        self.fps_graph_is_line = !self.fps_graph_is_line;
    }

    /// Toggle individual performance metrics
    pub fn toggle_fps_graph(&mut self) {
        self.show_fps_graph = !self.show_fps_graph;
    }

    pub fn toggle_tps_graph(&mut self) {
        self.show_tps_graph = !self.show_tps_graph;
    }

    pub fn toggle_frame_time_graph(&mut self) {
        self.show_frame_time_graph = !self.show_frame_time_graph;
    }

    pub fn toggle_memory_graph(&mut self) {
        self.show_memory_graph = !self.show_memory_graph;
    }

    pub fn toggle_draw_calls_graph(&mut self) {
        self.show_draw_calls_graph = !self.show_draw_calls_graph;
    }

    pub fn toggle_vertices_graph(&mut self) {
        self.show_vertices_graph = !self.show_vertices_graph;
    }

    pub fn toggle_input_latency_graph(&mut self) {
        self.show_input_latency_graph = !self.show_input_latency_graph;
    }

    pub fn toggle_ui_consistency_graph(&mut self) {
        self.show_ui_consistency_graph = !self.show_ui_consistency_graph;
    }

    // Setter methods for switches
    pub fn set_show_grid(&mut self, show: bool) {
        self.show_grid = show;
    }

    pub fn set_show_wireframe(&mut self, show: bool) {
        self.show_wireframe = show;
    }

    pub fn set_show_lighting(&mut self, show: bool) {
        self.show_lighting = show;
    }

    pub fn set_show_performance_overlay(&mut self, show: bool) {
        self.show_performance_overlay = show;
    }

    pub fn set_show_gpu_pipeline_overlay(&mut self, show: bool) {
        self.show_gpu_pipeline_overlay = show;
    }

    pub fn set_show_camera_mode_selector(&mut self, show: bool) {
        self.show_camera_mode_selector = show;
    }

    pub fn set_show_viewport_options(&mut self, show: bool) {
        self.show_viewport_options = show;
    }

    pub fn adjust_camera_move_speed(&mut self, delta: f32) {
        self.camera_move_speed = (self.camera_move_speed + delta).clamp(0.5, 100.0);
    }
}
