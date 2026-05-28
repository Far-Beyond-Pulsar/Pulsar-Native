//! Component Hierarchy Panel
//!
//! A tree view showing components attached to an object, using the generic hierarchical list.
//! Supports drag-and-drop reordering and nesting of components.
//!
//! ## Drag and Drop Controls
//! - **Drag onto component** - Nest the dragged component as a child (reparent)
//! - **Alt+Drag** - Reorder components at the same hierarchy level
//! - **Shift+Drag** - Remove parent (un-nest to root level)
//! - **Click chevron** - Expand/collapse components with children

use engine_backend::ComponentInstance;
use gpui::{prelude::*, *};
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    menu::popup_menu::PopupMenu,
    h_flex,
    HierarchicalTreeView, HierarchyConfig, HierarchyItem, HierarchyLayout,
    ActiveTheme, IconName, Sizable,
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

// ── Component Item ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ComponentItem {
    index: usize,
    instance: ComponentInstance,
    object_id: String,
    scene_db: SceneDatabase,
    state_arc: crate::level_editor::StateEntity,
    selected: bool,
    children_indices: Vec<usize>,
}

impl HierarchyItem for ComponentItem {
    type Id = usize;
    type DragPayload = ComponentDragPayload;

    fn id(&self) -> Self::Id {
        self.index
    }

    fn name(&self) -> String {
        if self.instance.enabled {
            self.instance.class_name.clone()
        } else {
            format!("{} (Disabled)", self.instance.class_name)
        }
    }

    fn icon(&self) -> IconName {
        IconName::Component
    }

    fn icon_color<V>(&self, _cx: &Context<V>) -> Hsla
    where
        V: Render,
    {
        use ui::hierarchical_tree::tree_colors;
        if self.instance.enabled {
            tree_colors::CODE_PURPLE
        } else {
            tree_colors::CODE_PURPLE.opacity(0.5)
        }
    }

    fn children_ids(&self) -> Vec<Self::Id> {
        self.children_indices.clone()
    }

    fn is_selected(&self) -> bool {
        self.selected
    }

    fn create_drag_payload(&self) -> Self::DragPayload {
        ComponentDragPayload {
            object_id: self.object_id.clone(),
            component_index: self.index,
            component_name: self.instance.class_name.clone(),
        }
    }

    fn drag_drop_id(&self) -> String {
        format!("comp-{}-{}", self.object_id, self.index)
    }

    fn extra_row_content<V>(&self, cx: &mut Context<V>) -> Option<AnyElement>
    where
        V: Render,
    {
        let scene_db = self.scene_db.clone();
        let toggle_object_id = self.object_id.clone();
        let index = self.index;
        let toggle_state = self.state_arc.clone();
        let enabled = self.instance.enabled;

        let toggle_button = Button::new(format!("component-toggle-{}-{}", toggle_object_id, index))
            .ghost()
            .xsmall()
            .icon(if enabled { IconName::Check } else { IconName::Xmark })
            .tooltip(if enabled { "Disable component" } else { "Enable component" })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                if scene_db.set_component_enabled(&toggle_object_id, index, !enabled) {
                    toggle_state.update(cx, |state, cx| {
                        state.scene_revision = state.scene_revision.saturating_add(1);
                        state.has_unsaved_changes = true;
                        cx.notify();
                    });
                }
            });

        Some(h_flex().gap_1().child(toggle_button).into_any_element())
    }

    fn build_context_menu(
        &self,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let duplicate_scene_db = self.scene_db.clone();
        let duplicate_object_id = self.object_id.clone();
        let duplicate_index = self.index;
        let duplicate_state = self.state_arc.clone();
        let delete_scene_db = self.scene_db.clone();
        let delete_object_id = self.object_id.clone();
        let delete_index = self.index;
        let delete_state = self.state_arc.clone();

        menu.menu_handler_with_icon("Duplicate", IconName::Copy, move |_, app| {
            if duplicate_scene_db.duplicate_component(&duplicate_object_id, duplicate_index).is_some() {
                duplicate_state.update(app, |state, cx| {
                    state.scene_revision = state.scene_revision.saturating_add(1);
                    state.has_unsaved_changes = true;
                    cx.notify();
                });
            }
        })
        .menu_handler_with_icon("Delete", IconName::Trash, move |_, app| {
            delete_scene_db.remove_component(&delete_object_id, delete_index);
            delete_state.update(app, |state, cx| {
                state.scene_revision = state.scene_revision.saturating_add(1);
                state.has_unsaved_changes = true;
                cx.notify();
            });
        })
    }
}

// ── Component Hierarchy Panel ─────────────────────────────────────────────────

/// Component Hierarchy - Shows all components in a tree structure
pub struct ComponentHierarchyPanel {
    object_id: String,
    scene_db: SceneDatabase,
}

impl ComponentHierarchyPanel {
    pub fn new(object_id: String, scene_db: SceneDatabase) -> Self {
        Self {
            object_id,
            scene_db,
        }
    }

    /// Get the parent index of a component from its data
    fn get_parent_index(component: &ComponentInstance) -> Option<usize> {
        component
            .data
            .get("__parent_index")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
    }

    /// Get child components of a given component index
    fn get_children(components: &[ComponentInstance], parent_index: usize) -> Vec<usize> {
        components
            .iter()
            .enumerate()
            .filter_map(|(idx, comp)| {
                if Self::get_parent_index(comp) == Some(parent_index) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Build component items from raw component instances
    fn build_items(
        &self,
        components: &[ComponentInstance],
        _selected_component: Option<usize>,
        state_arc: &crate::level_editor::StateEntity,
    ) -> Vec<ComponentItem> {
        components
            .iter()
            .enumerate()
            .map(|(idx, instance)| {
                let children_indices = Self::get_children(components, idx);
                ComponentItem {
                    index: idx,
                    instance: instance.clone(),
                    object_id: self.object_id.clone(),
                    scene_db: self.scene_db.clone(),
                    state_arc: state_arc.clone(),
                    selected: false, // TODO: Implement selection
                    children_indices,
                }
            })
            .collect()
    }

    pub fn render<V>(
        &self,
        state: &LevelEditorState,
        state_arc: crate::level_editor::StateEntity,
        add_button: AnyElement,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + Render,
    {
        let components = self.scene_db.get_components(&self.object_id);
        let items = self.build_items(&components, None, &state_arc);

        // Get root-level components (those without parents)
        let root_ids: Vec<usize> = components
            .iter()
            .enumerate()
            .filter_map(|(idx, comp)| {
                if Self::get_parent_index(comp).is_none() {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();

        let object_id = self.object_id.clone();
        let scene_db = self.scene_db.clone();
        let scene_db_for_root_drop = self.scene_db.clone();
        let state_arc_for_expand = state_arc.clone();
        let state_arc_for_nest = state_arc.clone();

        let config = HierarchyConfig {
            items,
            root_ids,
            layout: HierarchyLayout::Widget,

            // Header config (for Panel layout) - not used in Widget mode
            title: None,
            header_buttons: vec![],

            // Root drop zone
            root_drop_zone: Some((
                "Root".to_string(),
                Arc::new({
                    let object_id = self.object_id.clone();
                    move |payload: ComponentDragPayload| {
                        if payload.object_id != object_id {
                            return;
                        }
                        scene_db_for_root_drop.set_component_parent(
                            &object_id,
                            payload.component_index,
                            None,
                        );
                    }
                }),
            )),

            // Widget config
            widget_title: Some("Components".to_string()),
            widget_icon: Some(IconName::Component),
            widget_add_button: Some(add_button),
            empty_message: "No components — click + to add".to_string(),

            // Drag-and-drop options
            disable_nesting: false, // Allow component nesting

            // Callbacks
            is_expanded: Arc::new(move |idx: &usize| {
                state_arc_for_expand
                    .read(cx)
                    .expanded_components
                    .contains(&(object_id.clone(), *idx))
            }),
            on_toggle_expand: Arc::new({
                let object_id = self.object_id.clone();
                move |idx: &usize, _window, cx| {
                    let key = (object_id.clone(), *idx);
                    state_arc.update(cx, |state, cx| {
                        if state.expanded_components.contains(&key) {
                            state.expanded_components.remove(&key);
                        } else {
                            state.expanded_components.insert(key.clone());
                        }
                        cx.notify();
                    });
                }
            }),
            on_select: Arc::new(|_idx: &usize, _window, _cx| {
                // Component selection could be implemented here
            }),
            on_drop: Arc::new({
                let object_id = self.object_id.clone();
                move |payload: ComponentDragPayload, target_idx: &usize, modifiers: &Modifiers, _window, _cx| {
                    // Only allow operations within the same object
                    if payload.object_id != object_id {
                        return;
                    }

                    let from_idx = payload.component_index;
                    let to_idx = *target_idx;

                    if from_idx == to_idx {
                        return; // Can't drop onto self
                    }

                    // Check modifier keys to determine operation
                    if modifiers.shift {
                        // Remove parent - un-nest to root level
                        scene_db.set_component_parent(&object_id, from_idx, None);
                    } else if modifiers.alt {
                        // Reorder at same level
                        scene_db.reorder_component(&object_id, from_idx, to_idx);
                    } else {
                        // Default: nest the dragged component under the drop target
                        scene_db.set_component_parent(&object_id, from_idx, Some(to_idx));
                        // Auto-expand the parent to show the new child
                        state_arc_for_nest.update(_cx, |state, cx| {
                            state.expanded_components.insert((object_id.clone(), to_idx));
                            cx.notify();
                        });
                    }
                }
            }),
        };

        HierarchicalTreeView::new(config).render(cx)
    }
}
