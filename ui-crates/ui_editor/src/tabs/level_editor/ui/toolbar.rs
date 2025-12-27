use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _}, h_flex, ActiveTheme, IconName, Selectable, Sizable,
};
use std::sync::Arc;

use super::state::{LevelEditorState, TransformTool};
use crate::tabs::level_editor::scene_database::{ObjectType, MeshType, LightType};

/// Toolbar - Transform tools and quick actions
pub struct ToolbarPanel;

impl ToolbarPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V: 'static>(&self, state: &LevelEditorState, state_arc: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut Context<V>) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        h_flex()
            .w_full()
            .h(px(40.0))
            .px_3()
            .gap_1()
            .items_center()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                // Transform tools
                h_flex()
                    .gap_1()
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("tool_select")
                            .icon(IconName::CursorPointer)
                            .tooltip("Select (S)")
                            .selected(matches!(state.current_tool, TransformTool::Select))
                            .on_click(move |_, _, _| {
                                state_clone.write().set_tool(TransformTool::Select);
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("tool_move")
                            .icon(IconName::Drag)
                            .tooltip("Move (M)")
                            .selected(matches!(state.current_tool, TransformTool::Move))
                            .on_click(move |_, _, _| {
                                state_clone.write().set_tool(TransformTool::Move);
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("tool_rotate")
                            .icon(IconName::RotateCameraRight)
                            .tooltip("Rotate (R)")
                            .selected(matches!(state.current_tool, TransformTool::Rotate))
                            .on_click(move |_, _, _| {
                                state_clone.write().set_tool(TransformTool::Rotate);
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("tool_scale")
                            .icon(IconName::Enlarge)
                            .tooltip("Scale (T)")
                            .selected(matches!(state.current_tool, TransformTool::Scale))
                            .on_click(move |_, _, _| {
                                state_clone.write().set_tool(TransformTool::Scale);
                            })
                    })
            )
            .child(
                // Separator
                div()
                    .h_8()
                    .w_px()
                    .bg(cx.theme().border)
                    .mx_2()
            )
            .child(
                // Play/Stop controls
                h_flex()
                    .gap_1()
                    .child({
                        let state_clone = state_arc.clone();
                        if state.is_edit_mode() {
                            Button::new("play")
                                .icon(IconName::Play)
                                .tooltip("Play (Ctrl+P)")
                                .on_click(move |_, _, _| {
                                    state_clone.write().enter_play_mode();
                                })
                                .into_any_element()
                        } else {
                            Button::new("play_disabled")
                                .icon(IconName::Play)
                                .tooltip("Already playing")
                                .ghost()
                                .into_any_element()
                        }
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        if state.is_play_mode() {
                            Button::new("stop")
                                .icon(IconName::X)
                                .tooltip("Stop (Ctrl+.)")
                                .on_click(move |_, _, _| {
                                    state_clone.write().exit_play_mode();
                                })
                                .into_any_element()
                        } else {
                            Button::new("stop_disabled")
                                .icon(IconName::X)
                                .tooltip("Not playing")
                                .ghost()
                                .into_any_element()
                        }
                    })
            )
            .child(
                // Separator
                div()
                    .h_8()
                    .w_px()
                    .bg(cx.theme().border)
                    .mx_2()
            )
            .child(
                // Object creation tools
                h_flex()
                    .gap_1()
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("add_mesh")
                            .icon(IconName::Plus)
                            .tooltip("Add Mesh")
                            .on_click(move |_, _, _| {
                                use crate::tabs::level_editor::scene_database::{SceneObjectData, Transform};
                                let objects_count = state_clone.read().scene_objects().len();
                                let new_object = SceneObjectData {
                                    id: format!("mesh_{}", objects_count + 1),
                                    name: "New Mesh".to_string(),
                                    object_type: ObjectType::Mesh(MeshType::Cube),
                                    transform: Transform::default(),
                                    visible: true,
                                    locked: false,
                                    parent: None,
                                    children: vec![],
                                    components: vec![],
                                };
                                state_clone.read().scene_database.add_object(new_object, None);
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("add_light")
                            .icon(IconName::Sun)
                            .tooltip("Add Light")
                            .on_click(move |_, _, _| {
                                use crate::tabs::level_editor::scene_database::{SceneObjectData, Transform};
                                let objects_count = state_clone.read().scene_objects().len();
                                let new_object = SceneObjectData {
                                    id: format!("light_{}", objects_count + 1),
                                    name: "New Light".to_string(),
                                    object_type: ObjectType::Light(LightType::Directional),
                                    transform: Transform::default(),
                                    visible: true,
                                    locked: false,
                                    parent: None,
                                    children: vec![],
                                    components: vec![],
                                };
                                state_clone.read().scene_database.add_object(new_object, None);
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("add_camera")
                            .icon(IconName::Camera)
                            .tooltip("Add Camera")
                            .on_click(move |_, _, _| {
                                use crate::tabs::level_editor::scene_database::{SceneObjectData, Transform};
                                let objects_count = state_clone.read().scene_objects().len();
                                let new_object = SceneObjectData {
                                    id: format!("camera_{}", objects_count + 1),
                                    name: "New Camera".to_string(),
                                    object_type: ObjectType::Camera,
                                    transform: Transform::default(),
                                    visible: true,
                                    locked: false,
                                    parent: None,
                                    children: vec![],
                                    components: vec![],
                                };
                                state_clone.read().scene_database.add_object(new_object, None);
                            })
                    })
            )
            .child(
                // Spacer
                div().flex_1()
            )
            .child(
                // Scene file actions
                h_flex()
                    .gap_1()
                    .child({
                        let state_clone = state_arc.clone();
                        let mut btn = Button::new("save_scene")
                            .icon(IconName::FloppyDisk)
                            .tooltip("Save Scene (Ctrl+S)");

                        if state.has_unsaved_changes {
                            btn = btn.text_color(cx.theme().warning);
                        }

                        btn.on_click(move |_, _, _| {
                            let state_guard = state_clone.read();
                            
                            // Determine save path
                            let save_path = if let Some(ref path) = state_guard.current_scene {
                                path.clone()
                            } else {
                                // No current scene - save to default location
                                let scenes_dir = std::path::PathBuf::from("scenes");
                                if !scenes_dir.exists() {
                                    std::fs::create_dir_all(&scenes_dir).ok();
                                }
                                
                                // Generate timestamped filename
                                let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
                                scenes_dir.join(format!("scene_{}.json", timestamp))
                            };
                            
                            // Save the scene
                            match state_guard.scene_database.save_to_file(&save_path) {
                                Ok(_) => {
                                    tracing::info!("[LEVEL-EDITOR] 💾 Scene saved: {:?}", save_path);
                                    drop(state_guard); // Release read lock before write
                                    state_clone.write().current_scene = Some(save_path);
                                    state_clone.write().has_unsaved_changes = false;
                                }
                                Err(e) => {
                                    tracing::info!("[LEVEL-EDITOR] ❌ Failed to save scene: {}", e);
                                }
                            }
                        })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("new_scene")
                            .icon(IconName::FolderPlus)
                            .tooltip("New Scene (Ctrl+N)")
                            .on_click(move |_, _, _| {
                                state_clone.write().scene_database.clear();
                                state_clone.write().scene_database = crate::tabs::level_editor::SceneDatabase::with_default_scene();
                                state_clone.write().current_scene = None;
                                state_clone.write().has_unsaved_changes = false;
                                tracing::info!("[LEVEL-EDITOR] 📄 New scene created");
                            })
                    })
            )
    }
}
