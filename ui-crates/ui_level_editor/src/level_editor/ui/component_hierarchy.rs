//! Component Hierarchy Panel
//!
//! A tree view showing components attached to an object, similar to the scene hierarchy.
//! Supports drag-and-drop reordering and nesting of components.

use engine_backend::ComponentInstance;
use gpui::{prelude::*, *};
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

use crate::level_editor::scene_database::SceneDatabase;

// ── Drag Payload ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ComponentDragPayload {
    pub object_id: String,
    pub component_index: usize,
    pub component_name: String,
}

impl Render for ComponentDragPayload {
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
            .child(self.component_name.clone())
    }
}

/// Component Hierarchy - Shows all components in a tree structure
pub struct ComponentHierarchyPanel {
    object_id: String,
    scene_db: SceneDatabase,
    /// Selected component index
    selected_component: Option<usize>,
}

impl ComponentHierarchyPanel {
    pub fn new(object_id: String, scene_db: SceneDatabase) -> Self {
        Self {
            object_id,
            scene_db,
            selected_component: None,
        }
    }

    pub fn render<V>(
        &self,
        add_button: AnyElement,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + Render,
    {
        let components = self.scene_db.get_components(&self.object_id);
        let component_count = components.len();

        v_flex()
            .w_full()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                // Header
                h_flex()
                    .w_full()
                    .px_3()
                    .py(px(6.0))
                    .justify_between()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::Component)
                                    .small()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child("Components"),
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
                                    .child(format!("{}", component_count)),
                            ),
                    )
                    .child(add_button),
            )
            .child(
                // Scrollable component list
                v_flex()
                    .flex_1()
                    .w_full()
                    .max_h(px(300.0))
                    .gap_px()
                    .p_1()
                    .scrollable(ScrollbarAxis::Vertical)
                    .when(components.is_empty(), |el| {
                        el.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("No components — click + to add"),
                        )
                    })
                    .children(components.iter().enumerate().map(|(idx, component)| {
                        self.render_component_tree_item(idx, component, 0, cx)
                    })),
            )
    }

    fn render_component_tree_item<V>(
        &self,
        idx: usize,
        component: &ComponentInstance,
        depth: usize,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + Render,
    {
        let is_selected = self.selected_component == Some(idx);
        let indent = px(depth as f32 * 20.0 + 4.0);
        let class_name = component.class_name.clone();
        let object_id = self.object_id.clone();

        // Text colors based on selection state
        let text_color = if is_selected {
            cx.theme().accent_foreground
        } else {
            cx.theme().foreground
        };

        // For now, components don't have children (we can add nested component support later)
        let has_children = false;
        let expand_arrow: AnyElement = if has_children {
            div()
                .w_4()
                .h_4()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.0))
                        .text_color(cx.theme().muted_foreground),
                )
                .into_any_element()
        } else {
            div().w_4().into_any_element()
        };

        // Drag payload
        let drag_payload = ComponentDragPayload {
            object_id: object_id.clone(),
            component_index: idx,
            component_name: class_name.clone(),
        };

        // Drop area for reordering
        let scene_db_for_drop = self.scene_db.clone();
        let obj_id_for_drop = object_id.clone();

        DropArea::<ComponentDragPayload>::new(format!("component-drop-{}", idx))
            .on_drop(move |payload, _window, _cx| {
                // Only allow reordering within the same object
                if payload.object_id == obj_id_for_drop {
                    let from_idx = payload.component_index;
                    let to_idx = idx;
                    if from_idx != to_idx {
                        scene_db_for_drop.reorder_component(&obj_id_for_drop, from_idx, to_idx);
                    }
                }
            })
            .child(
                Draggable::new(format!("component-drag-{}", idx), drag_payload)
                    .drag_handle(DragHandlePosition::Left)
                    .w_full()
                    .child(
                    // Row content
                    h_flex()
                        .id(SharedString::from(format!("component-{}", idx)))
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
                        .child(expand_arrow)
                        // Component icon
                        .child(
                            div()
                                .w_5()
                                .h_5()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.0))
                                .bg(cx.theme().accent.opacity(0.15))
                                .child(
                                    Icon::new(IconName::Component)
                                        .size(px(14.0))
                                        .text_color(if is_selected {
                                            text_color
                                        } else {
                                            cx.theme().accent
                                        }),
                                ),
                        )
                        // Component class name
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(text_color)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(class_name),
                        )
                        // Delete button (shown when selected)
                        .when(is_selected, |row| {
                            let scene_db = self.scene_db.clone();
                            let obj_id = self.object_id.clone();
                            row.child(
                                Button::new(format!("delete-component-{}", idx))
                                    .icon(IconName::Trash)
                                    .xsmall()
                                    .ghost()
                                    .on_click(move |_, _, _| {
                                        scene_db.remove_component(&obj_id, idx);
                                    }),
                            )
                        }),
                ),
            )
    }
}
