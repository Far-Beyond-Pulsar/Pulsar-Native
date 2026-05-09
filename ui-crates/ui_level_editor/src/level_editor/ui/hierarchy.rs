use gpui::{prelude::*, *};
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    hierarchical_tree::tree_colors,
    ActiveTheme, Icon, IconName, Sizable, StyledExt,
};

use super::hierarchical_list::{
    HierarchicalTreeView, HierarchyConfig, HierarchyItem, HierarchyLayout,
};
use super::state::{HierarchyDragPayload, LevelEditorState, SceneObject};
use crate::level_editor::scene_database::{ObjectType, SceneDatabase};

/// GPUI Render impl for the hierarchy drag ghost label.
impl Render for HierarchyDragPayload {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded(px(4.0))
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().foreground)
            .child(self.object_name.clone())
    }
}

// ── Scene Object Item ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct SceneObjectItem {
    object: SceneObject,
    scene_db: SceneDatabase,
    is_selected: bool,
    is_folder: bool,
}

impl HierarchyItem for SceneObjectItem {
    type Id = String;
    type DragPayload = HierarchyDragPayload;

    fn id(&self) -> Self::Id {
        self.object.id.clone()
    }

    fn name(&self) -> String {
        self.object.name.clone()
    }

    fn icon(&self) -> IconName {
        HierarchyPanel::get_icon_for_object_type(self.object.object_type)
    }

    fn icon_color<V>(&self, cx: &Context<V>) -> Hsla
    where
        V: Render,
    {
        HierarchyPanel::get_icon_color_for_type(self.object.object_type, cx)
    }

    fn children_ids(&self) -> Vec<Self::Id> {
        self.object.children.clone()
    }

    fn is_selected(&self) -> bool {
        self.is_selected
    }

    fn create_drag_payload(&self) -> Self::DragPayload {
        HierarchyDragPayload {
            object_id: self.object.id.clone(),
            object_name: self.object.name.clone(),
        }
    }

    fn drag_drop_id(&self) -> String {
        format!("hierarchy-{}", self.object.id)
    }

    fn extra_row_content<V>(&self, cx: &mut Context<V>) -> Option<AnyElement>
    where
        V: Render,
    {
        let scene_db = self.scene_db.clone();
        let object_id = self.object.id.clone();
        let is_visible = self.object.visible;

        Some(
            div()
                .w_5()
                .h_5()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().muted.opacity(0.3)))
                .child(
                    Icon::new(if is_visible {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .size(px(12.0))
                    .text_color(if is_visible {
                        if self.is_selected {
                            cx.theme().accent_foreground.opacity(0.7)
                        } else {
                            cx.theme().muted_foreground
                        }
                    } else {
                        cx.theme().danger
                    }),
                )
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    if let Some(mut obj) = scene_db.get_object(&object_id) {
                        obj.visible = !obj.visible;
                        scene_db.update_object(obj);
                    }
                })
                .into_any_element(),
        )
    }

    fn on_click_custom(&self) -> Option<Arc<dyn Fn()>> {
        if self.is_folder {
            // Folders use expand/collapse instead of selection
            None
        } else {
            None
        }
    }
}

/// Hierarchy Panel - Scene outliner showing all objects in a tree structure
pub struct HierarchyPanel;

impl HierarchyPanel {
    pub fn new() -> Self {
        Self
    }

    fn build_items(
        all_objects: &[SceneObject],
        selected_object: Option<&String>,
        scene_db: &SceneDatabase,
    ) -> Vec<SceneObjectItem> {
        all_objects
            .iter()
            .map(|obj| {
                let is_selected = selected_object == Some(&obj.id);
                let is_folder = matches!(obj.object_type, ObjectType::Folder);
                SceneObjectItem {
                    object: obj.clone(),
                    scene_db: scene_db.clone(),
                    is_selected,
                    is_folder,
                }
            })
            .collect()
    }

    pub fn render<V>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        add_button: AnyElement,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<PanelEvent> + Render,
    {
        let all_objects = state.scene_database.get_all_objects();
        let items = Self::build_items(
            &all_objects,
            state.selected_object().as_ref(),
            &state.scene_database,
        );

        // Root-level objects (those without parents)
        let root_ids: Vec<String> = state
            .scene_objects()
            .iter()
            .map(|obj| obj.id.clone())
            .collect();

        let state_arc_for_expand = state_arc.clone();
        let state_arc_for_select = state_arc.clone();
        let state_arc_for_drop = state_arc.clone();
        let state_arc_for_root_drop = state_arc.clone();
        let state_arc_for_root_click = state_arc.clone();

        // Header buttons
        let header_buttons = vec![
            add_button,
            {
                let state_clone = state_arc.clone();
                Button::new("add_folder")
                    .icon(IconName::FolderPlus)
                    .ghost()
                    .xsmall()
                    .tooltip(t!("LevelEditor.Hierarchy.AddFolder"))
                    .on_click(move |_, _, _| {
                        use crate::level_editor::commands::{execute_command, SceneCommand};
                        use crate::level_editor::scene_database::{
                            ObjectType, SceneObjectData, Transform,
                        };
                        let mut state = state_clone.write();
                        execute_command(
                            &mut state,
                            SceneCommand::AddObject {
                                data: SceneObjectData {
                                    id: String::new(),
                                    name: "New Folder".to_string(),
                                    object_type: ObjectType::Folder,
                                    transform: Transform::default(),
                                    visible: true,
                                    locked: false,
                                    parent: None,
                                    children: vec![],
                                    components: vec![],
                                    scene_path: String::new(),
                                },
                                parent_id: None,
                            },
                        );
                    })
                    .into_any_element()
            },
            {
                let state_clone = state_arc.clone();
                Button::new("delete_object")
                    .icon(IconName::Trash)
                    .ghost()
                    .xsmall()
                    .tooltip(t!("LevelEditor.Hierarchy.DeleteSelected"))
                    .on_click(move |_, _, _| {
                        use crate::level_editor::commands::{execute_command, SceneCommand};
                        if let Some(id) = state_clone.read().selected_object() {
                            let mut state = state_clone.write();
                            execute_command(&mut state, SceneCommand::RemoveObject { id });
                        }
                    })
                    .into_any_element()
            },
        ];

        let config = HierarchyConfig {
            items,
            root_ids,
            layout: HierarchyLayout::Panel,

            // Panel header
            title: Some(t!("LevelEditor.Hierarchy.Title").to_string()),
            header_buttons,

            // Root drop zone
            root_drop_zone: Some((
                "Root".to_string(),
                Arc::new(move |payload: HierarchyDragPayload| {
                    use crate::level_editor::commands::{execute_command, SceneCommand};
                    let mut state = state_arc_for_root_drop.write();
                    execute_command(
                        &mut state,
                        SceneCommand::ReparentObject {
                            id: payload.object_id,
                            new_parent_id: None,
                        },
                    );
                }),
            )),

            // Widget config (not used in Panel mode)
            widget_title: None,
            widget_icon: None,
            widget_add_button: None,
            empty_message: String::new(),

            // Callbacks
            is_expanded: Arc::new(move |id: &String| {
                state_arc_for_expand.read().is_object_expanded(id)
            }),
            on_toggle_expand: Arc::new(move |id: &String| {
                let mut state = state_arc.write();
                if state.is_object_expanded(id) {
                    state.expanded_objects.remove(id);
                } else {
                    state.expanded_objects.insert(id.clone());
                }
            }),
            on_select: Arc::new(move |id: &String| {
                state_arc_for_select.write().select_object(Some(id.clone()));
            }),
            on_drop: Arc::new(
                move |payload: HierarchyDragPayload, target_id: &String, modifiers: &Modifiers| {
                    use crate::level_editor::commands::{execute_command, SceneCommand};
                    if payload.object_id == *target_id {
                        return;
                    }
                    let mut state = state_arc_for_drop.write();
                    if modifiers.shift {
                        execute_command(
                            &mut state,
                            SceneCommand::ReparentObject {
                                id: payload.object_id,
                                new_parent_id: None,
                            },
                        );
                    } else if modifiers.alt {
                        // Reorder doesn't fit a SceneCommand variant yet — call directly and
                        // bump revision so the polling task propagates the change.
                        state
                            .scene_database
                            .reorder_object_siblings(&payload.object_id, target_id);
                        state.scene_revision = state.scene_revision.saturating_add(1);
                    } else {
                        let result = execute_command(
                            &mut state,
                            SceneCommand::ReparentObject {
                                id: payload.object_id,
                                new_parent_id: Some(target_id.clone()),
                            },
                        );
                        if result.changed {
                            state.expanded_objects.insert(target_id.clone());
                        }
                    }
                },
            ),
        };

        HierarchicalTreeView::new(config).render(cx)
    }

    pub fn get_icon_for_object_type(object_type: ObjectType) -> IconName {
        match object_type {
            ObjectType::Camera => IconName::Camera,
            ObjectType::Folder => IconName::Folder,
            ObjectType::Light(_) => IconName::LightBulb,
            ObjectType::Mesh(_) => IconName::Box,
            ObjectType::Empty => IconName::Circle,
            ObjectType::ParticleSystem => IconName::Sparks,
            ObjectType::AudioSource => IconName::MusicNote,
        }
    }

    pub fn get_icon_color_for_type<V>(object_type: ObjectType, cx: &Context<V>) -> Hsla
    where
        V: Render,
    {
        match object_type {
            ObjectType::Camera => tree_colors::CODE_BLUE,
            ObjectType::Folder => tree_colors::FOLDER,
            ObjectType::Light(_) => tree_colors::SPECIAL_YELLOW,
            ObjectType::Mesh(_) => tree_colors::CODE_PURPLE,
            ObjectType::Empty => cx.theme().muted_foreground,
            ObjectType::ParticleSystem => tree_colors::EFFECT_ORANGE,
            ObjectType::AudioSource => tree_colors::DOC_TEAL,
        }
    }
}

use ui::dock::PanelEvent;
