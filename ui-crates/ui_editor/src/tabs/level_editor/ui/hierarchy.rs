use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    context_menu::ContextMenuExt,
    h_flex, v_flex, scroll::ScrollbarAxis, ActiveTheme, Icon, IconName, Sizable, StyledExt,
};
use std::sync::Arc;

use super::state::{LevelEditorState, SceneObject};
use super::actions::*;
use crate::tabs::level_editor::scene_database::ObjectType;

/// Hierarchy Panel - Scene outliner showing all objects in a tree structure
pub struct HierarchyPanel;

impl HierarchyPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>
    ) -> impl IntoElement
    where
        V: EventEmitter<PanelEvent> + Render,
    {
        use super::state::HierarchyDragState;
        let is_drag_in_progress = !matches!(&state.hierarchy_drag_state, HierarchyDragState::None);

        let state_arc_for_esc = state_arc.clone();

        v_flex()
            .size_full()
            .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                // ESC to cancel drag
                if event.keystroke.key.as_str() == "escape" {
                    let mut state = state_arc_for_esc.write();
                    if !matches!(state.hierarchy_drag_state, HierarchyDragState::None) {
                        println!("[HIERARCHY] ❌ Drag cancelled");
                        state.hierarchy_drag_state = HierarchyDragState::None;
                        cx.notify();
                    }
                }
            }))
            .child(
                // Header
                div()
                    .w_full()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("Hierarchy")
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child({
                                        let state_clone = state_arc.clone();
                                        Button::new("add_object")
                                            .icon(IconName::Plus)
                                            .ghost()
                                            .xsmall()
                                            .tooltip("Add Object")
                                            .on_click(move |_, _, _| {
                                                use crate::tabs::level_editor::scene_database::{SceneObjectData, ObjectType, Transform};
                                                let objects_count = state_clone.read().scene_objects().len();
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
                                                state_clone.read().scene_database.add_object(new_object, None);
                                            })
                                    })
                                    .child({
                                        let state_clone = state_arc.clone();
                                        Button::new("add_folder")
                                            .icon(IconName::FolderPlus)
                                            .ghost()
                                            .xsmall()
                                            .tooltip("Add Folder")
                                            .on_click(move |_, _, _| {
                                                use crate::tabs::level_editor::scene_database::{SceneObjectData, ObjectType, Transform};
                                                let objects_count = state_clone.read().scene_objects().len();
                                                let new_folder = SceneObjectData {
                                                    id: format!("folder_{}", objects_count + 1),
                                                    name: "New Folder".to_string(),
                                                    object_type: ObjectType::Folder,
                                                    transform: Transform::default(),
                                                    visible: true,
                                                    locked: false,
                                                    parent: None,
                                                    children: vec![],
                                                    components: vec![],
                                                };
                                                state_clone.read().scene_database.add_object(new_folder, None);
                                            })
                                    })
                                    .child({
                                        let state_clone = state_arc.clone();
                                        Button::new("delete_object")
                                            .icon(IconName::Trash)
                                            .ghost()
                                            .xsmall()
                                            .tooltip("Delete Selected")
                                            .on_click(move |_, _, _| {
                                                if let Some(id) = state_clone.read().selected_object() {
                                                    state_clone.read().scene_database.remove_object(&id);
                                                }
                                            })
                                    })
                            )
                    )
            )
            .child(
                // Object tree
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child({
                        let mut tree_container = v_flex()
                            .size_full()
                            .scrollable(ScrollbarAxis::Vertical)
                            .children(
                                state.scene_objects().iter().map(|obj| {
                                    Self::render_object_tree_item(obj, state, state_arc.clone(), 0, cx)
                                })
                            );

                        // Add root-level drop zone at the bottom if dragging
                        if is_drag_in_progress {
                            let state_clone_for_root_drop = state_arc.clone();
                            tree_container = tree_container.child(
                                div()
                                    .w_full()
                                    .h(px(32.0))
                                    .mt_2()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .border_2()
                                    .border_dashed()
                                    .border_color(cx.theme().border)
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Drop here to make root-level")
                                    .on_mouse_up(MouseButton::Left, move |_, _, _| {
                                        let mut state_write = state_clone_for_root_drop.write();
                                        if let HierarchyDragState::DraggingObject { object_id: dragged_id, .. } = &state_write.hierarchy_drag_state {
                                            let dragged_id = dragged_id.clone();
                                            let success = state_write.scene_database.reparent_object(&dragged_id, None);
                                            if success {
                                                println!("[HIERARCHY] 🏠 Made '{}' a root-level object", dragged_id);
                                            }
                                            state_write.hierarchy_drag_state = HierarchyDragState::None;
                                        }
                                    })
                                    .hover(|style| {
                                        style.bg(cx.theme().primary.opacity(0.1))
                                            .border_color(cx.theme().primary)
                                    })
                            );
                        }

                        tree_container
                    })
            )
    }

    fn render_object_tree_item<V: 'static>(
        object: &SceneObject,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        depth: usize,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<PanelEvent> + Render,
    {
        let is_selected = state.selected_object().as_ref() == Some(&object.id);
        let has_children = !object.children.is_empty();
        let is_expanded = state.is_object_expanded(&object.id);
        let indent = px(depth as f32 * 16.0);
        let icon = Self::get_icon_for_object_type(object.object_type);
        let object_id = object.id.clone();
        let object_id_for_expand = object.id.clone();

        // Check drag state
        use super::state::HierarchyDragState;
        let is_being_dragged = matches!(&state.hierarchy_drag_state,
            HierarchyDragState::DraggingObject { object_id: id, .. } if id == &object.id);
        let is_drag_in_progress = !matches!(&state.hierarchy_drag_state, HierarchyDragState::None);

        // Build item div base
        let item_id = SharedString::from(format!("object-{}", object.id));
        let mut item_div = div()
            .id(item_id)
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .h(px(24.0))
            .pl(indent + px(8.0))
            .pr_2()
            .rounded_md()
            .cursor_pointer();

        // Apply conditional styling
        if is_being_dragged {
            // Dragged item - semi-transparent
            item_div = item_div
                .bg(cx.theme().accent.opacity(0.5))
                .border_2()
                .border_color(cx.theme().accent);
        } else if is_selected {
            item_div = item_div.bg(cx.theme().accent);
        } else {
            item_div = item_div.hover(|style| style.bg(cx.theme().accent.opacity(0.1)));
        }

        let state_clone_for_click = state_arc.clone();
        let object_id_for_click = object_id.clone();
        let state_clone_for_drag_start = state_arc.clone();
        let object_id_for_drag_start = object_id.clone();
        let state_clone_for_drop = state_arc.clone();
        let object_id_for_drop = object_id.clone();

        // Container for the entire item (for drop target)
        let mut container = div()
            .w_full()
            .flex()
            .flex_col();

        // Add drop target highlighting if drag is in progress and this isn't the dragged item
        if is_drag_in_progress && !is_being_dragged {
            container = container
                .on_mouse_move(move |_, _, _| {
                    // Visual feedback handled by hover state
                })
                .on_mouse_up(MouseButton::Left, move |_, _, _| {
                    // Handle drop
                    let mut state_write = state_clone_for_drop.write();
                    if let HierarchyDragState::DraggingObject { object_id: dragged_id, .. } = &state_write.hierarchy_drag_state {
                        let dragged_id = dragged_id.clone();
                        let target_id = object_id_for_drop.clone();

                        // Reparent the dragged object to the target (make it a child of target)
                        if dragged_id != target_id {
                            let success = state_write.scene_database.reparent_object(&dragged_id, Some(target_id.clone()));
                            if success {
                                println!("[HIERARCHY] 🎯 Reparented '{}' as child of '{}'", dragged_id, target_id);
                                // Expand the target to show the new child
                                state_write.expanded_objects.insert(target_id);
                            }
                        }

                        // Clear drag state
                        state_write.hierarchy_drag_state = HierarchyDragState::None;
                    }
                })
                .hover(|style| {
                    // Highlight drop target
                    style.bg(cx.theme().primary.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().primary)
                });
        }

        container.child(
                item_div
                    .on_mouse_down(MouseButton::Left, move |event, _, _| {
                        // Select on click
                        state_clone_for_click.write().select_object(Some(object_id_for_click.clone()));

                        // Start drag operation (only if shift is held to differentiate from click)
                        if event.modifiers.shift {
                            let mut state_write = state_clone_for_drag_start.write();
                            let parent = state_write.scene_database.get_object(&object_id_for_drag_start)
                                .and_then(|obj| obj.parent.clone());
                            state_write.hierarchy_drag_state = HierarchyDragState::DraggingObject {
                                object_id: object_id_for_drag_start.clone(),
                                original_parent: parent,
                            };
                            println!("[HIERARCHY] 🖱️ Started dragging '{}'", object_id_for_drag_start);
                        }
                    })
                    .child(
                        // Expand/collapse arrow for items with children
                        if has_children {
                            div()
                                .w_4()
                                .text_xs()
                                .text_color(if is_selected {
                                    cx.theme().accent_foreground
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(if is_expanded { "▼" } else { "▶" })
                                .on_mouse_down(MouseButton::Left, cx.listener(move |view, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.dispatch_action(&ToggleObjectExpanded {
                                        object_id: object_id_for_expand.clone()
                                    });
                                }))
                                .into_any_element()
                        } else {
                            div()
                                .w_4()
                                .into_any_element()
                        }
                    )
                    .child(Icon::new(icon).size_4())
                    .child({
                        let mut text_div = div().text_sm();
                        if is_selected {
                            text_div = text_div.text_color(cx.theme().accent_foreground);
                        } else {
                            text_div = text_div.text_color(cx.theme().foreground);
                        }
                        text_div.child(object.name.clone())
                    })
                    .child(
                        // Visibility toggle
                        div()
                            .ml_auto()
                            .text_xs()
                            .text_color(if object.visible {
                                if is_selected {
                                    cx.theme().accent_foreground.opacity(0.7)
                                } else {
                                    cx.theme().muted_foreground
                                }
                            } else {
                                cx.theme().danger
                            })
                            .child(if object.visible { "●" } else { "○" })
                    )
            )
            // Render children recursively if expanded
            .children(
                if has_children && is_expanded {
                    object.children.iter()
                        .filter_map(|child_id| state.scene_database.get_object(child_id))
                        .map(|child_obj| {
                            Self::render_object_tree_item(
                                &child_obj,
                                state,
                                state_arc.clone(),
                                depth + 1,  // Indent one level deeper
                                cx
                            )
                        })
                        .collect::<Vec<_>>()
                } else {
                    vec![]
                }
            )
    }

    fn get_icon_for_object_type(object_type: ObjectType) -> IconName {
        match object_type {
            ObjectType::Camera => IconName::Camera,
            ObjectType::Folder => IconName::Folder,
            ObjectType::Light(_) => IconName::LightBulb,
            ObjectType::Mesh(_) => IconName::Box,
            ObjectType::Empty => IconName::Circle,
            ObjectType::ParticleSystem => IconName::Play,
            ObjectType::AudioSource => IconName::MusicNote,
        }
    }
}

use ui::dock::PanelEvent;
