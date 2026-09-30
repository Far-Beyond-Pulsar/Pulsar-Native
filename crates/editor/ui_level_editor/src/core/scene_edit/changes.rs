//! Which component properties were written since the properties panel last
//! looked.
//!
//! The panel drains this once per frame via [`drain_property_changes`] and uses
//! the result to skip world reads for unchanged properties, the single biggest
//! cost reduction for it. It is process-wide because there is exactly one
//! editor scene per process; it holds no scene data, only "this was touched".

use std::collections::HashSet;
use std::sync::LazyLock;

use parking_lot::Mutex;

/// Soft cap on the accumulated change set (see [`record_property_change`]).
const MAX_PROPERTY_CHANGE_SET: usize = 16_384;

/// Tracks which specific properties have been written since the last drain.
#[derive(Default, Clone)]
pub struct PropertyChangeSet {
    /// `(object_id, class_name, prop_name)` triples written since last drain.
    changed: HashSet<(String, String, String)>,
    /// `(object_id, class_name)` pairs where a structural change occurred
    /// (add/remove/reorder/enable-disable) — the component list itself changed.
    structural: HashSet<(String, String)>,
    /// `true` when any component was added or removed on the target object,
    /// meaning the panel should rebuild its component card list entirely.
    components_added_or_removed: bool,
}

impl PropertyChangeSet {
    /// `true` if *no* property was written since the last drain.
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.structural.is_empty()
    }

    /// `true` when the component *list* changed (add/remove), not just a
    /// property value within an existing component.
    pub fn components_added_or_removed(&self) -> bool {
        self.components_added_or_removed
    }

    /// Check if a specific property was written since the last drain.
    pub fn has_changed(&self, object_id: &str, class_name: &str, prop_name: &str) -> bool {
        self.changed.contains(&(
            object_id.to_string(),
            class_name.to_string(),
            prop_name.to_string(),
        ))
    }

    /// Check if any property on a given class was written since the last drain.
    pub fn class_changed(&self, object_id: &str, class_name: &str) -> bool {
        self.changed
            .iter()
            .any(|(oid, cls, _)| oid == object_id && cls == class_name)
    }

    /// Did anything touching `object_id`'s components change since the last drain?
    fn touches_object(&self, object_id: &str) -> bool {
        self.components_added_or_removed
            || self.changed.iter().any(|(oid, _, _)| oid == object_id)
            || self.structural.iter().any(|(oid, _)| oid == object_id)
    }
}

static CHANGES: LazyLock<Mutex<PropertyChangeSet>> = LazyLock::new(Mutex::default);

/// Snapshot and clear the accumulated property changes. Called exactly once per
/// properties-panel render to decide which values need re-reading from the world.
pub fn drain_property_changes() -> PropertyChangeSet {
    std::mem::take(&mut *CHANGES.lock())
}

/// Non-consuming check: has anything touching `object_id`'s components been
/// written since the last [`drain_property_changes`]? This is the properties
/// panel's relevance gate; peeking keeps the drain contract with the section's
/// own render.
pub fn has_property_changes_for(object_id: &str) -> bool {
    CHANGES.lock().touches_object(object_id)
}

/// Record that a specific property was written.
pub(super) fn record_property_change(object_id: &str, class_name: &str, prop_name: &str) {
    let mut changes = CHANGES.lock();
    // Soft cap: with relevance-gated rendering the set can go several drains'
    // worth of edits without being emptied. Dropping the history past this
    // point is always *safe* — an empty set just makes the panel fall back to
    // reading values it might have skipped.
    if changes.changed.len() >= MAX_PROPERTY_CHANGE_SET {
        *changes = PropertyChangeSet::default();
    }
    changes.changed.insert((
        object_id.to_string(),
        class_name.to_string(),
        prop_name.to_string(),
    ));
}

/// Record a structural change (add/remove/reorder/enable-disable) on a component.
pub(super) fn record_structural_change(object_id: &str, class_name: &str) {
    let mut changes = CHANGES.lock();
    if changes.changed.len() >= MAX_PROPERTY_CHANGE_SET {
        *changes = PropertyChangeSet::default();
    }
    changes
        .structural
        .insert((object_id.to_string(), class_name.to_string()));
    changes.components_added_or_removed = true;
}
