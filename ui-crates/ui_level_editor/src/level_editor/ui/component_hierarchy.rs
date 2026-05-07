//! Component Hierarchy Panel
//!
//! A tree view showing components attached to an object, similar to the scene hierarchy.
//! Supports drag-and-drop reordering and nesting of components.
//!
//! ## Drag and Drop Controls
//! - **Drag onto component** - Nest the dragged component as a child (reparent)
//! - **Alt+Drag** - Reorder components at the same hierarchy level
//! - **Shift+Drag** - Remove parent (un-nest to root level)
//! - **Click chevron** - Expand/collapse components with children

use engine_backend::ComponentInstance;
use gpui::{prelude::*, *};
use std::collections::HashSet;
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
use crate::level_editor::ui::state::LevelEditorState;

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

    /// Get the parent index of a component from its data
    fn get_parent_index(&self, component: &ComponentInstance) -> Option<usize> {
        component
            .data
            .get("__parent_index")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
    }

    /// Get child components of a given component index
    fn get_children(&self, components: &[ComponentInstance], parent_index: usize) -> Vec<usize> {
        components
            .iter()
            .enumerate()
            .filter_map(|(idx, comp)| {
                if self.get_parent_index(comp) == Some(parent_index) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get root-level components (those without parents)
    fn get_root_components(&self, components: &[ComponentInstance]) -> Vec<usize> {
        components
            .iter()
            .enumerate()
            .filter_map(|(idx, comp)| {
                if self.get_parent_index(comp).is_none() {
                    Some(idx)
                } else {
                    None
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
        V: 'static + Render,
    {
        let components = self.scene_db.get_components(&self.object_id);
        let component_count = components.len();
        let object_id = self.object_id.clone();

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
                    .children({
                        let root_indices = self.get_root_components(&components);
                        root_indices.into_iter().map(|idx| {
                            self.render_component_tree_item(
                                idx,
                                &components[idx],
                                &components,
                                0,
                                state,
                                state_arc.clone(),
                                cx,
                            )
                        })
                    }),
            )
    }

    fn render_component_tree_item<V>(
        &self,
        idx: usize,
        component: &ComponentInstance,
        all_components: &[ComponentInstance],
        depth: usize,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + Render,
    {
        let is_selected = self.selected_component == Some(idx);
        let indent = px(depth as f32 * 20.0 + 4.0);
        let class_name = component.class_name.clone();
        let object_id = self.object_id.clone();
        let component_key = (object_id.clone(), idx);

        // Text colors based on selection state
        let text_color = if is_selected {
            cx.theme().accent_foreground
        } else {
            cx.theme().foreground
        };

        // Check if this component has children
        let children = self.get_children(all_components, idx);
        let has_children = !children.is_empty();
        let is_expanded = state.expanded_components.contains(&component_key);

        // Expand/collapse arrow
        let expand_arrow: AnyElement = if has_children {
            let state_for_expand = state_arc.clone();
            let key_for_expand = component_key.clone();
            div()
                .w_4()
                .h_4()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().muted.opacity(0.5)))
                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                    let mut state_write = state_for_expand.write();
                    if state_write.expanded_components.contains(&key_for_expand) {
                        state_write.expanded_components.remove(&key_for_expand);
                    } else {
                        state_write.expanded_components.insert(key_for_expand.clone());
                    }
                })
                .child(
                    Icon::new(if is_expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
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

        // Drop area for reordering and nesting
        let scene_db_for_drop = self.scene_db.clone();
        let obj_id_for_drop = object_id.clone();
        let drop_target_idx = idx;
        let state_for_drop = state_arc.clone();

        DropArea::<ComponentDragPayload>::new(format!("component-drop-{}", idx))
            .on_drop(move |payload, window, _cx| {
                // Only allow operations within the same object
                if payload.object_id != obj_id_for_drop {
                    return;
                }

                let from_idx = payload.component_index;
                let to_idx = drop_target_idx;

                if from_idx == to_idx {
                    return; // Can't drop onto self
                }

                // Check modifier keys to determine operation:
                // - Regular drag = nest as child (reparent)
                // - Alt+drag = reorder at same level
                // - Shift+drag = remove parent (un-nest to root)
                let modifiers = window.modifiers();
                if modifiers.shift {
                    // Remove parent - un-nest to root level
                    scene_db_for_drop.set_component_parent(
                        &obj_id_for_drop,
                        from_idx,
                        None,
                    );
                } else if modifiers.alt {
                    // Reorder at same level
                    scene_db_for_drop.reorder_component(&obj_id_for_drop, from_idx, to_idx);
                } else {
                    // Default: nest the dragged component under the drop target
                    scene_db_for_drop.set_component_parent(
                        &obj_id_for_drop,
                        from_idx,
                        Some(to_idx),
                    );
                    // Auto-expand the parent to show the new child
                    state_for_drop.write().expanded_components.insert((obj_id_for_drop.clone(), to_idx));
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
                                        Icon::new(IconName::Component).size(px(14.0)).text_color(
                                            if is_selected {
                                                text_color
                                            } else {
                                                cx.theme().accent
                                            },
                                        ),
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
            // Recursively render children if expanded
            .when(has_children && is_expanded, |parent| {
                parent.children(children.into_iter().map(|child_idx| {
                    self.render_component_tree_item(
                        child_idx,
                        &all_components[child_idx],
                        all_components,
                        depth + 1,
                        state,
                        state_arc.clone(),
                        cx,
                    )
                }))
            })
    }
}
