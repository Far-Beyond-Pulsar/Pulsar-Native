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
use ui::{ActiveTheme, IconName};

use super::hierarchical_list::{
    HierarchicalTreeView, HierarchyConfig, HierarchyItem, HierarchyLayout,
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
        self.instance.class_name.clone()
    }

    fn icon(&self) -> IconName {
        IconName::Component
    }

    fn icon_color<V>(&self, _cx: &Context<V>) -> Hsla
    where
        V: Render,
    {
        use ui::hierarchical_tree::tree_colors;
        tree_colors::CODE_PURPLE
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
                    selected: false, // TODO: Implement selection
                    children_indices,
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
        let items = self.build_items(&components, None);

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
        let state_arc_for_expand = state_arc.clone();
        let state_arc_for_nest = state_arc.clone();

        let config = HierarchyConfig {
            items,
            root_ids,
            layout: HierarchyLayout::Widget,

            // Header config (for Panel layout) - not used in Widget mode
            title: None,
            header_buttons: vec![],

            // Root drop zone - not used
            root_drop_zone: None,

            // Widget config
            widget_title: Some("Components".to_string()),
            widget_icon: Some(IconName::Component),
            widget_add_button: Some(add_button),
            empty_message: "No components — click + to add".to_string(),

            // Callbacks
            is_expanded: Arc::new(move |idx: &usize| {
                state_arc_for_expand
                    .read()
                    .expanded_components
                    .contains(&(object_id.clone(), *idx))
            }),
            on_toggle_expand: Arc::new({
                let object_id = self.object_id.clone();
                move |idx: &usize| {
                    let mut state = state_arc.write();
                    let key = (object_id.clone(), *idx);
                    if state.expanded_components.contains(&key) {
                        state.expanded_components.remove(&key);
                    } else {
                        state.expanded_components.insert(key);
                    }
                }
            }),
            on_select: Arc::new(|_idx: &usize| {
                // Component selection could be implemented here
            }),
            on_drop: Arc::new({
                let object_id = self.object_id.clone();
                move |payload: ComponentDragPayload, target_idx: &usize, modifiers: &Modifiers| {
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
                        state_arc_for_nest
                            .write()
                            .expanded_components
                            .insert((object_id.clone(), to_idx));
                    }
                }
            }),
        };

        HierarchicalTreeView::new(config).render(cx)
    }
}
