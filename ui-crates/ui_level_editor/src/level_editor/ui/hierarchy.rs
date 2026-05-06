use gpui::{prelude::*, *};
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    draggable::{DragHandlePosition, Draggable},
    drop_area::DropArea,
    h_flex,
    hierarchical_tree::tree_colors,
    scroll::ScrollbarAxis,
    v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt,
};

use super::state::{HierarchyDragPayload, LevelEditorState, SceneObject};
use crate::level_editor::scene_database::ObjectType;

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

/// Hierarchy Panel - Scene outliner showing all objects in a tree structure
pub struct HierarchyPanel;

impl HierarchyPanel {
    pub fn new() -> Self {
        Self
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
        let object_count = state.scene_database.get_all_objects().len();

        v_flex()
            .size_full()
            .bg(cx.theme().background)
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
                                    .child(t!("LevelEditor.Hierarchy.Title").to_string()),
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
                                    .child(format!("{} objects", object_count)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(add_button)
                            .child({
                                let state_clone = state_arc.clone();
                                Button::new("add_folder")
                                    .icon(IconName::FolderPlus)
                                    .ghost()
                                    .xsmall()
                                    .tooltip(t!("LevelEditor.Hierarchy.AddFolder"))
                                    .on_click(move |_, _, _| {
                                        use crate::level_editor::scene_database::{
                                            ObjectType, SceneObjectData, Transform,
                                        };
                                        let objects_count =
                                            state_clone.read().scene_objects().len();
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
                                            scene_path: String::new(),
                                        };
                                        state_clone
                                            .read()
                                            .scene_database
                                            .add_object(new_folder, None);
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
                            }),
                    ),
            )
            .child(
                // Object tree with proper scroll container
                div().flex_1().overflow_hidden().child({
                    let state_arc_root_drop = state_arc.clone();
                    let state_arc_root_click = state_arc.clone();
                    v_flex()
                        .size_full()
                        .p_2()
                        .gap_2()
                        .child(
                            DropArea::<HierarchyDragPayload>::new("hierarchy-root-drop")
                                .on_drop(move |payload, _window, _cx| {
                                    state_arc_root_drop
                                        .read()
                                        .scene_database
                                        .reparent_object(&payload.object_id, None);
                                })
                                .child(
                                    h_flex()
                                        .w_full()
                                        .items_center()
                                        .gap_1()
                                        .h_7()
                                        .pl(px(8.0))
                                        .pr_2()
                                        .rounded(px(4.0))
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().muted.opacity(0.18))
                                        .child(
                                            Icon::new(IconName::Folder)
                                                .size(px(14.0))
                                                .text_color(tree_colors::FOLDER),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Root"),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_event, _window, _cx| {
                                                state_arc_root_click.write().select_object(None);
                                            },
                                        ),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .w_full()
                                .gap_px()
                                .scrollable(ScrollbarAxis::Vertical)
                                .children(state.scene_objects().iter().map(|obj| {
                                    Self::render_object_tree_item(
                                        obj,
                                        state,
                                        state_arc.clone(),
                                        0,
                                        cx,
                                    )
                                })),
                        )
                }),
            )
    }

    fn render_object_tree_item<V>(
        object: &SceneObject,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        depth: usize,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<PanelEvent> + Render,
    {
        let is_selected = state.selected_object().as_ref() == Some(&object.id);
        let has_children = !object.children.is_empty();
        let is_expanded = state.is_object_expanded(&object.id);
        let is_folder = matches!(object.object_type, ObjectType::Folder);
        let indent = px(depth as f32 * 20.0 + 4.0);
        let icon = Self::get_icon_for_object_type(object.object_type);
        let icon_color = Self::get_icon_color_for_type(object.object_type, cx);
        let object_id = object.id.clone();

        // Text colors based on selection state.
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

        // ── Expand/collapse arrow ─────────────────────────────────────────
        let expand_arrow: AnyElement = if has_children {
            let state_for_expand = state_arc.clone();
            let id_for_expand = object_id.clone();
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
                    Icon::new(if is_expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(12.0))
                    .text_color(muted_color),
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
            div().w_4().into_any_element()
        };

        // ── Row content (icon + name + eye) ──────────────────────────────
        let state_clone_for_click = state_arc.clone();
        let object_id_for_click = object_id.clone();

        let row_content = h_flex()
            .id(SharedString::from(format!("object-{}", object.id)))
            .w_full()
            .items_center()
            .gap_1()
            .h_7()
            .pl(indent)
            .pr_2()
            .rounded(px(4.0))
            .cursor_pointer()
            .when(is_selected, |s| s.bg(cx.theme().accent).shadow_sm())
            .when(!is_selected, |s| {
                s.hover(|style| style.bg(cx.theme().muted.opacity(0.3)))
            })
            .on_click(cx.listener(move |_view, _event, _window, cx| {
                if is_folder {
                    let mut state_write = state_clone_for_click.write();
                    if state_write.is_object_expanded(&object_id_for_click) {
                        state_write.expanded_objects.remove(&object_id_for_click);
                    } else {
                        state_write
                            .expanded_objects
                            .insert(object_id_for_click.clone());
                    }
                } else {
                    state_clone_for_click
                        .write()
                        .select_object(Some(object_id_for_click.clone()));
                }
                cx.notify();
            }))
            .child(expand_arrow)
            // Type icon
            .child(
                div()
                    .w_5()
                    .h_5()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.0))
                    .bg(icon_color.opacity(0.15))
                    .child(Icon::new(icon).size(px(14.0)).text_color(if is_selected {
                        text_color
                    } else {
                        icon_color
                    })),
            )
            // Name
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(text_color)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(object.name.clone()),
            )
            // Visibility eye
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
                        Icon::new(if object.visible {
                            IconName::Eye
                        } else {
                            IconName::EyeOff
                        })
                        .size(px(12.0))
                        .text_color(if object.visible {
                            muted_color
                        } else {
                            cx.theme().danger
                        }),
                    ),
            );

        // ── Drag source wrapper ───────────────────────────────────────────
        let drag_payload = HierarchyDragPayload {
            object_id: object_id.clone(),
            object_name: object.name.clone(),
        };

        let draggable_row = Draggable::new(format!("hierarchy-drag-{}", object_id), drag_payload)
            .drag_handle(DragHandlePosition::Left)
            .w_full()
            .child(row_content);

        // ── Drop target wrapper (folders accept any hierarchy item) ───────
        let state_arc_for_drop = state_arc.clone();
        let drop_target_id = object_id.clone();

        let drop_row =
            DropArea::<HierarchyDragPayload>::new(format!("hierarchy-drop-{}", object_id))
                .can_accept(move |payload| payload.object_id != drop_target_id)
                .on_drop({
                    let drop_target_id2 = object_id.clone();
                    move |payload, _window, _cx| {
                        if payload.object_id != drop_target_id2 {
                            // Use a single write lock for both operations to avoid
                            // a deadlock (read then write on the same RwLock).
                            let mut state = state_arc_for_drop.write();
                            let success = state
                                .scene_database
                                .reparent_object(&payload.object_id, Some(drop_target_id2.clone()));
                            if success {
                                state.expanded_objects.insert(drop_target_id2.clone());
                            }
                        }
                    }
                })
                .w_full()
                .child(draggable_row);

        // ── Compose: drop zone + children ─────────────────────────────────
        let children: Vec<AnyElement> = if has_children && is_expanded {
            object
                .children
                .iter()
                .filter_map(|child_id| state.scene_database.get_object(child_id))
                .map(|child_obj| {
                    Self::render_object_tree_item(
                        &child_obj,
                        state,
                        state_arc.clone(),
                        depth + 1,
                        cx,
                    )
                    .into_any_element()
                })
                .collect()
        } else {
            vec![]
        };

        v_flex().w_full().child(drop_row).children(children)
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
