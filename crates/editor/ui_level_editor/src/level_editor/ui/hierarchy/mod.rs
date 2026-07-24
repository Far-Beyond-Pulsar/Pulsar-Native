pub mod tree_item_renderer;

use crate::level_editor::scene_database::SceneObjectData;
use crate::level_editor::scene_database::{ObjectType, SceneDatabase};
use crate::level_editor::state::{HierarchyDragPayload, LevelEditorState};
use gpui::{prelude::*, *};
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    hierarchical_tree::tree_colors,
    menu::popup_menu::PopupMenu,
    ActiveTheme, HierarchicalTreeView, HierarchyConfig, HierarchyItem, HierarchyLayout, Icon,
    IconName, Sizable, StyledExt,
};

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
    object: SceneObjectData,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
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
        if self
            .object
            .props
            .get("icon_asset")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            return IconName::Image;
        }
        HierarchyPanel::get_icon_for_object_type(self.object.object_type)
    }

    fn icon_color<V>(&self, cx: &Context<V>) -> Hsla
    where
        V: Render,
    {
        if self
            .object
            .props
            .get("icon_asset")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            return tree_colors::DOC_TEAL;
        }
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
        let visibility_id = self.object.id.clone();
        let visibility_state = self.state_arc.clone();
        let is_visible = self.object.visible;

        let duplicate_id = self.object.id.clone();
        let duplicate_state = self.state_arc.clone();
        let delete_id = self.object.id.clone();
        let delete_state = self.state_arc.clone();

        let visibility_button = Button::new(format!("scene-visibility-{}", visibility_id))
            .ghost()
            .xsmall()
            .icon(if is_visible {
                IconName::Eye
            } else {
                IconName::EyeOff
            })
            .tooltip(if is_visible {
                "Hide object"
            } else {
                "Show object"
            })
            .on_click(move |_, _, cx| {
                use crate::level_editor::commands::{execute_command, SceneCommand};
                let mut state = visibility_state.write();
                if let Some(mut obj) = state.scene.database.get_object(&visibility_id) {
                    obj.visible = !obj.visible;
                    execute_command(&mut state, SceneCommand::UpdateObject { data: obj });
                }
                drop(state);
                cx.stop_propagation();
            });

        let duplicate_button = Button::new(format!("scene-duplicate-{}", duplicate_id))
            .ghost()
            .xsmall()
            .icon(IconName::Copy)
            .tooltip("Duplicate object")
            .on_click(move |_, _, cx| {
                use crate::level_editor::commands::{execute_command, SceneCommand};
                let mut state = duplicate_state.write();
                execute_command(
                    &mut state,
                    SceneCommand::DuplicateObject {
                        source_id: duplicate_id.clone(),
                        count: 1,
                        position_offset: None,
                    },
                );
                drop(state);
                cx.stop_propagation();
            });

        let delete_button = Button::new(format!("scene-delete-{}", delete_id))
            .ghost()
            .xsmall()
            .icon(IconName::Trash)
            .tooltip("Delete object")
            .on_click(move |_, _, cx| {
                use crate::level_editor::commands::{execute_command, SceneCommand};
                let mut state = delete_state.write();
                execute_command(
                    &mut state,
                    SceneCommand::RemoveObject {
                        id: delete_id.clone(),
                    },
                );
                drop(state);
                cx.stop_propagation();
            });

        Some(
            h_flex()
                .gap_0p5()
                .child(visibility_button)
                .child(duplicate_button)
                .child(delete_button)
                .into_any_element(),
        )
    }

    fn build_context_menu(
        &self,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        use crate::level_editor::commands::{execute_command, SceneCommand};

        let duplicate_id = self.object.id.clone();
        let delete_id = self.object.id.clone();
        let duplicate_state = self.state_arc.clone();
        let delete_state = self.state_arc.clone();

        menu.menu_handler_with_icon("Duplicate", IconName::Copy, move |_, app| {
            let _ = app;
            let mut state = duplicate_state.write();
            execute_command(
                &mut state,
                SceneCommand::DuplicateObject {
                    source_id: duplicate_id.clone(),
                    count: 1,
                    position_offset: None,
                },
            );
        })
        .menu_handler_with_icon("Delete", IconName::Trash, move |_, app| {
            let _ = app;
            let mut state = delete_state.write();
            execute_command(
                &mut state,
                SceneCommand::RemoveObject {
                    id: delete_id.clone(),
                },
            );
        })
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
        all_objects: &[SceneObjectData],
        selected_object: Option<&String>,
        state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
    ) -> Vec<SceneObjectItem> {
        all_objects
            .iter()
            .map(|obj| {
                let is_selected = selected_object == Some(&obj.id);
                let is_folder = matches!(obj.object_type, ObjectType::Folder);
                SceneObjectItem {
                    object: obj.clone(),
                    state_arc: state_arc.clone(),
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
        wrapper_entity: WeakEntity<V>,
        add_button: AnyElement,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<PanelEvent> + Render,
    {
        let all_objects = state.scene.database.get_all_objects();
        let items = Self::build_items(
            &all_objects,
            state.scene.selected_object().as_ref(),
            &state_arc,
        );

        // Root-level objects (those without parents)
        let root_ids: Vec<String> = state
            .scene
            .scene_objects()
            .iter()
            .map(|obj| obj.id.clone())
            .collect();

        let state_arc_for_expand = state_arc.clone();
        let state_arc_for_toggle = state_arc.clone();
        let wrapper_for_toggle = wrapper_entity.clone();
        let state_arc_for_select = state_arc.clone();
        let wrapper_for_select = wrapper_entity.clone();
        let state_arc_for_drop = state_arc.clone();
        let wrapper_for_drop = wrapper_entity.clone();
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
                                    scene_path: String::new(),
                                    props: Default::default(),
                                    component_instances: None,
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
                        if let Some(id) = state_clone.read().scene.selected_object() {
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

            // Root drop zone - NOTE: This callback doesn't have access to cx, so it still uses RwLock
            // TODO: Update DropArea to pass window and cx to callbacks
            root_drop_zone: Some((
                "Root".to_string(),
                Arc::new(move |payload: HierarchyDragPayload| {
                    use crate::level_editor::commands::{execute_command, SceneCommand};
                    let mut state = state_arc.write();
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

            // Drag-and-drop options
            disable_nesting: false, // Allow full nesting in hierarchy

            // Callbacks
            is_expanded: Arc::new(move |id: &String| {
                state_arc_for_expand.read().hierarchy.is_object_expanded(id)
            }),
            on_toggle_expand: Arc::new(move |id: &String, _window, cx| {
                let id = id.clone();
                let state = state_arc_for_toggle.clone();
                let wrapper = wrapper_for_toggle.clone();
                cx.defer(move |cx| {
                    let mut state = state.write();
                    if state.hierarchy.is_object_expanded(&id) {
                        state.hierarchy.expanded_objects.remove(&id);
                    } else {
                        state.hierarchy.expanded_objects.insert(id);
                    }
                    drop(state);
                    if let Some(wrapper) = wrapper.upgrade() {
                        cx.notify(wrapper.entity_id());
                    }
                });
            }),
            on_select: Arc::new(move |id: &String, _window, cx| {
                let id = id.clone();
                let state = state_arc_for_select.clone();
                let wrapper = wrapper_for_select.clone();
                cx.defer(move |cx| {
                    {
                        let mut guard = state.write();
                        guard.scene.select_object(Some(id));
                    }
                    if let Some(wrapper) = wrapper.upgrade() {
                        cx.notify(wrapper.entity_id());
                    }
                });
            }),
            on_drop: Arc::new(
                move |payload: HierarchyDragPayload,
                      target_id: &String,
                      modifiers: &Modifiers,
                      _window,
                      cx| {
                    use crate::level_editor::commands::{execute_command, SceneCommand};
                    if payload.object_id == *target_id {
                        return;
                    }
                    let object_id = payload.object_id.clone();
                    let target_id = target_id.clone();
                    let mods = modifiers.clone();
                    let state = state_arc_for_drop.clone();
                    let wrapper = wrapper_for_drop.clone();

                    cx.defer(move |cx| {
                        let mut state = state.write();
                        if mods.shift {
                            execute_command(
                                &mut state,
                                SceneCommand::ReparentObject {
                                    id: object_id,
                                    new_parent_id: None,
                                },
                            );
                        } else if mods.alt {
                            // Reorder doesn't fit a SceneCommand variant yet — call directly and
                            // bump revision so the polling task propagates the change.
                            state
                                .scene
                                .database
                                .reorder_object_siblings(&object_id, &target_id);
                            state.scene.revision = state.scene.revision.saturating_add(1);
                        } else {
                            let result = execute_command(
                                &mut state,
                                SceneCommand::ReparentObject {
                                    id: object_id,
                                    new_parent_id: Some(target_id.clone()),
                                },
                            );
                            if result.changed {
                                state.hierarchy.expanded_objects.insert(target_id);
                            }
                        }
                        drop(state);
                        if let Some(wrapper) = wrapper.upgrade() {
                            cx.notify(wrapper.entity_id());
                        }
                    });
                },
            ),
        };

        cx.new(|cx| HierarchicalTreeView::new(config, cx)).into_any_element()
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
            ObjectType::Blueprint => IconName::Code,
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
            ObjectType::Blueprint => tree_colors::CODE_BLUE,
        }
    }
}

use ui::dock::PanelEvent;
