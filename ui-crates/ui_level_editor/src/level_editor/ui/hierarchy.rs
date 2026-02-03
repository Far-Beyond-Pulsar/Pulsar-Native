use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    context_menu::ContextMenuExt,
    h_flex, v_flex, scroll::ScrollbarAxis, ActiveTheme, Icon, IconName, Sizable, StyledExt,
    hierarchical_tree::{render_tree_folder, tree_colors},
};
use std::sync::Arc;
use rust_i18n::t;

use super::state::{LevelEditorState, SceneObject};
use super::actions::*;
use crate::level_editor::scene_database::ObjectType;

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
        let object_count = state.scene_database.get_root_objects().len();

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                // ESC to cancel drag
                if event.keystroke.key.as_str() == "escape" {
                    let mut state = state_arc_for_esc.write();
                    if !matches!(state.hierarchy_drag_state, HierarchyDragState::None) {
                        tracing::debug!("[HIERARCHY] ❌ Drag cancelled");
                        state.hierarchy_drag_state = HierarchyDragState::None;
                        cx.notify();
                    }
                }
            }))
            .child(
                // Professional header
                h_flex()
                    .w_full()
                    .px_4()
                    .py_3()
                    .justify_between()
                    .items_center()
                    .bg(cx.theme().sidebar)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child(t!("LevelEditor.Hierarchy.Title").to_string())
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(cx.theme().muted.opacity(0.5))
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} objects", object_count))
                            )
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
                                    .tooltip(t!("LevelEditor.Hierarchy.AddObject"))
                                    .on_click(move |_, _, _| {
                                        use crate::level_editor::scene_database::{SceneObjectData, ObjectType, Transform};
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
                                    .tooltip(t!("LevelEditor.Hierarchy.AddFolder"))
                                    .on_click(move |_, _, _| {
                                        use crate::level_editor::scene_database::{SceneObjectData, ObjectType, Transform};
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
                                    .tooltip(t!("LevelEditor.Hierarchy.DeleteSelected"))
                                    .on_click(move |_, _, _| {
                                        if let Some(id) = state_clone.read().selected_object() {
                                            state_clone.read().scene_database.remove_object(&id);
                                        }
                                    })
                            })
                    )
            )
            .child(
                // Object tree with proper scroll container
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child({
                        let mut tree_container = v_flex()
                            .size_full()
                            .p_2()
                            .gap_px()
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
                                    .rounded(px(6.0))
                                    .border_2()
                                    .border_dashed()
                                    .border_color(cx.theme().border)
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(t!("LevelEditor.Hierarchy.DropHere").to_string())
                                    .on_mouse_up(MouseButton::Left, move |_, _, _| {
                                        let mut state_write = state_clone_for_root_drop.write();
                                        if let HierarchyDragState::DraggingObject { object_id: dragged_id, .. } = &state_write.hierarchy_drag_state {
                                            let dragged_id = dragged_id.clone();
                                            let success = state_write.scene_database.reparent_object(&dragged_id, None);
                                            if success {
                                                tracing::debug!("[HIERARCHY] 🏠 Made '{}' a root-level object", dragged_id);
                                            }
                                            state_write.hierarchy_drag_state = HierarchyDragState::None;
                                        }
                                    })
                                    .hover(|style| {
                                        style.bg(cx.theme().accent.opacity(0.1))
                                            .border_color(cx.theme().accent)
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
        let is_folder = matches!(object.object_type, ObjectType::Folder);
        let indent = px(depth as f32 * 20.0 + 4.0);
        let icon = Self::get_icon_for_object_type(object.object_type);
        let icon_color = Self::get_icon_color_for_type(object.object_type, cx);
        let object_id = object.id.clone();
        let object_id_for_expand = object.id.clone();

        // Check drag state
        use super::state::HierarchyDragState;
        let is_being_dragged = matches!(&state.hierarchy_drag_state,
            HierarchyDragState::DraggingObject { object_id: id, .. } if id == &object.id);
        let is_drag_in_progress = !matches!(&state.hierarchy_drag_state, HierarchyDragState::None);

        // Build item div base
        let item_id = SharedString::from(format!("object-{}", object.id));
        let mut item_div = h_flex()
            .id(item_id)
            .w_full()
            .items_center()
            .gap_1()
            .h_7()
            .pl(indent)
            .pr_2()
            .rounded(px(4.0))
            .cursor_pointer();

        // Apply conditional styling
        if is_being_dragged {
            // Dragged item - semi-transparent with accent border
            item_div = item_div
                .bg(cx.theme().accent.opacity(0.3))
                .border_1()
                .border_color(cx.theme().accent);
        } else if is_selected {
            // Selected item - accent background
            item_div = item_div
                .bg(cx.theme().accent)
                .shadow_sm();
        } else {
            // Normal item - subtle hover
            item_div = item_div.hover(|style| style.bg(cx.theme().muted.opacity(0.3)));
        }

        let state_clone_for_click = state_arc.clone();
        let object_id_for_click = object_id.clone();
        let state_clone_for_drag_start = state_arc.clone();
        let object_id_for_drag_start = object_id.clone();
        let state_clone_for_drop = state_arc.clone();
        let object_id_for_drop = object_id.clone();
        let state_clone_for_expand = state_arc.clone();
        let object_id_for_expand_click = object_id_for_expand.clone();

        // Container for the entire item (for drop target)
        let mut container = v_flex()
            .w_full();

        // Add drop target highlighting if drag is in progress and this isn't the dragged item
        if is_drag_in_progress && !is_being_dragged && is_folder {
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
                                tracing::debug!("[HIERARCHY] 🎯 Reparented '{}' as child of '{}'", dragged_id, target_id);
                                // Expand the target to show the new child
                                state_write.expanded_objects.insert(target_id);
                            }
                        }

                        // Clear drag state
                        state_write.hierarchy_drag_state = HierarchyDragState::None;
                    }
                })
                .hover(|style| {
                    // Highlight drop target for folders
                    style.bg(cx.theme().accent.opacity(0.15))
                        .rounded(px(4.0))
                });
        }

        // Text color based on selection state
        let text_color = if is_selected {
            cx.theme().accent_foreground
        } else {
            cx.theme().foreground
        };

        let muted_color = if is_selected {
            cx.theme().accent_foreground.opacity(0.7)
        } else {
            cx.theme().muted_foreground
        };

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
                            tracing::debug!("[HIERARCHY] 🖱️ Started dragging '{}'", object_id_for_drag_start);
                        }
                    })
                    // Expand/collapse arrow for items with children
                    .child(
                        if has_children {
                            let state_for_expand = state_clone_for_expand.clone();
                            let id_for_expand = object_id_for_expand_click.clone();
                            div()
                                .w_4()
                                .h_4()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(2.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(cx.theme().muted.opacity(0.5)))
                                .child(
                                    Icon::new(if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                        .size(px(12.0))
                                        .text_color(muted_color)
                                )
                                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                                    cx.stop_propagation();
                                    let mut state_write = state_for_expand.write();
                                    if state_write.is_object_expanded(&id_for_expand) {
                                        state_write.expanded_objects.remove(&id_for_expand);
                                    } else {
                                        state_write.expanded_objects.insert(id_for_expand.clone());
                                    }
                                })
                                .into_any_element()
                        } else {
                            div()
                                .w_4()
                                .into_any_element()
                        }
                    )
                    // Icon with type-specific coloring
                    .child(
                        div()
                            .w_5()
                            .h_5()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .bg(icon_color.opacity(0.15))
                            .child(
                                Icon::new(icon)
                                    .size(px(14.0))
                                    .text_color(if is_selected { text_color } else { icon_color })
                            )
                    )
                    // Object name
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(text_color)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(object.name.clone())
                    )
                    // Visibility indicator
                    .child(
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
                                Icon::new(if object.visible { IconName::Eye } else { IconName::EyeOff })
                                    .size(px(12.0))
                                    .text_color(if object.visible { muted_color } else { cx.theme().danger })
                            )
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
            ObjectType::ParticleSystem => IconName::Sparks,
            ObjectType::AudioSource => IconName::MusicNote,
        }
    }

    fn get_icon_color_for_type<V>(object_type: ObjectType, cx: &Context<V>) -> Hsla {
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
