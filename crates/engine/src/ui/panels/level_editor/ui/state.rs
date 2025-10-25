use std::path::PathBuf;
use std::collections::HashSet;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// Import our scene database and gizmo systems
use crate::ui::panels::level_editor::{SceneDatabase, GizmoState, GizmoType};

/// Editor mode - Edit or Play
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Edit,  // Editing mode - gizmos active, game thread paused
    Play,  // Play mode - game running, gizmos hidden
}

/// Shared state for the Level Editor
#[derive(Clone)]
pub struct LevelEditorState {
    /// Scene database - single source of truth for all scene data
    pub scene_database: SceneDatabase,
    /// Snapshot of scene state when entering play mode (for reset on stop)
    pub scene_snapshot: Option<Arc<parking_lot::RwLock<Vec<crate::ui::panels::level_editor::scene_database::SceneObjectData>>>>,
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
    pub show_viewport_controls: bool,
    pub show_camera_mode_selector: bool,
    pub show_viewport_options: bool,
    /// FPS graph type (true = line, false = bar)
    pub fps_graph_is_line: bool,
    /// Expanded state for hierarchy items
    pub expanded_objects: HashSet<String>,
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
pub use crate::ui::panels::level_editor::scene_database::{
    ObjectType as SceneObjectType,
    Transform,
    SceneObjectData as SceneObject,
    LightType,
    MeshType,
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
            show_performance_overlay: true,
            show_viewport_controls: true,
            show_camera_mode_selector: true,
            show_viewport_options: true,
            fps_graph_is_line: true,
            expanded_objects: HashSet::new(),
        }
    }
}

impl LevelEditorState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter play mode - snapshot scene and start game thread
    pub fn enter_play_mode(&mut self) {
        println!("[EDITOR] 🎮 Entering PLAY mode");
        
        // Save snapshot of current scene state for restoration
        let objects = self.scene_database.get_all_objects();
        self.scene_snapshot = Some(Arc::new(parking_lot::RwLock::new(objects)));
        
        // Switch to play mode
        self.editor_mode = EditorMode::Play;
        
        // Hide gizmos
        let mut gizmo = self.gizmo_state.write();
        gizmo.set_gizmo_type(GizmoType::None);
        
        println!("[EDITOR] ✅ Play mode active - game thread will start");
    }

    /// Exit play mode - restore scene state and stop game thread
    pub fn exit_play_mode(&mut self) {
        println!("[EDITOR] 🛑 Exiting PLAY mode");
        
        // Restore scene from snapshot
        if let Some(ref snapshot) = self.scene_snapshot {
            let objects = snapshot.read().clone();
            
            // Clear current scene
            self.scene_database.clear();
            
            // Restore all objects
            for obj in objects {
                self.scene_database.add_object(obj, None);
            }
            
            println!("[EDITOR] ✅ Scene restored from snapshot");
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
        
        println!("[EDITOR] ✅ Edit mode active");
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
        if let Some(id) = object_id {
            let mut gizmo = self.gizmo_state.write();
            gizmo.target_object_id = Some(id);
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
        
        // Update gizmo type
        let gizmo_type = match tool {
            TransformTool::Select => GizmoType::None,
            TransformTool::Move => GizmoType::Translate,
            TransformTool::Rotate => GizmoType::Rotate,
            TransformTool::Scale => GizmoType::Scale,
        };
        
        let mut gizmo = self.gizmo_state.write();
        gizmo.set_gizmo_type(gizmo_type);
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

    /// Toggle viewport controls
    pub fn toggle_viewport_controls(&mut self) {
        self.show_viewport_controls = !self.show_viewport_controls;
    }

    /// Toggle camera mode selector
    pub fn toggle_camera_mode_selector(&mut self) {
        self.show_camera_mode_selector = !self.show_camera_mode_selector;
    }

    /// Toggle viewport options
    pub fn toggle_viewport_options(&mut self) {
        self.show_viewport_options = !self.show_viewport_options;
    }

    /// Toggle FPS graph type
    pub fn toggle_fps_graph_type(&mut self) {
        self.fps_graph_is_line = !self.fps_graph_is_line;
    }
}
