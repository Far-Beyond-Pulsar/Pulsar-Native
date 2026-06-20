//! Hierarchy Domain — expand/collapse state and drag-and-drop tracking for
//! the scene object hierarchy tree.
//!
//! This state is purely **transient**: it is never persisted to disk and resets
//! when the editor is closed. Separating it from [`SceneDomain`](super::scene::SceneDomain)
//! keeps the scene data model clean of UI ephemera.

use std::collections::HashSet;

// ── Hierarchy drag state ──────────────────────────────────────────────────

/// Payload for GPUI-native hierarchy drag-and-drop.
#[derive(Clone, Debug)]
pub struct HierarchyDragPayload {
    pub object_id: String,
    pub object_name: String,
}

/// Tracks an in-progress hierarchy drag operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HierarchyDragState {
    None,
    DraggingObject {
        object_id: String,
        original_parent: Option<String>,
    },
}

/// Hierarchy UI state — purely transient view-state for the tree panel.
#[derive(Clone)]
pub struct HierarchyDomain {
    /// Object IDs that are expanded in the hierarchy tree.
    pub expanded_objects: HashSet<String>,
    /// Expanded component entries keyed by `(object_id, component_index)`.
    pub expanded_components: HashSet<(String, usize)>,
    /// Active drag operation state.
    pub drag_state: HierarchyDragState,
}

impl Default for HierarchyDomain {
    fn default() -> Self {
        Self {
            expanded_objects: HashSet::new(),
            expanded_components: HashSet::new(),
            drag_state: HierarchyDragState::None,
        }
    }
}

impl HierarchyDomain {
    pub fn toggle_object_expanded(&mut self, object_id: &str) {
        if self.expanded_objects.contains(object_id) {
            self.expanded_objects.remove(object_id);
        } else {
            self.expanded_objects.insert(object_id.to_string());
        }
    }

    pub fn is_object_expanded(&self, object_id: &str) -> bool {
        self.expanded_objects.contains(object_id)
    }

    pub fn expand_all(&mut self) {
        // This is a convenience stub — the caller should pass root objects.
    }

    pub fn collapse_all(&mut self) {
        self.expanded_objects.clear();
    }
}
