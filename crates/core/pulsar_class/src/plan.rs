//! Which prefab component goes where when a class is instantiated.
//!
//! SceneDB keeps one component per type per entity, so:
//!
//! - the first copy of each component type goes on the instance root;
//! - a second copy of a type already on the root, or a component that
//!   carries its own transform (`__transform` in its data), goes on its own
//!   child object. A child whose prefab parent (`__parent_index`) is also a
//!   child is nested under it.
//!
//! Every planned component carries its slot id as `__slot_id` metadata;
//! that is how slot → entity/component is looked up afterwards
//! ([`crate::world::slot_map`]).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::component::ClassInstance;
use crate::overrides::{apply, normalize_component_data, split_meta};
use crate::registry::ClassDefinition;
use crate::{PARENT_INDEX_KEY, REMOVED_KEY, SLOT_ID_KEY, TRANSFORM_KEY};

/// A component's local transform relative to the instance root.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalTransform {
    #[serde(default)]
    pub position: [f32; 3],
    /// Euler degrees, the editor's convention.
    #[serde(default)]
    pub rotation: [f32; 3],
    #[serde(default = "one")]
    pub scale: [f32; 3],
}

fn one() -> [f32; 3] {
    [1.0; 3]
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

impl LocalTransform {
    pub fn from_value(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    pub fn to_value(self) -> Value {
        json!({ "position": self.position, "rotation": self.rotation, "scale": self.scale })
    }
}

/// One component to create.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedComponent {
    pub slot_id: String,
    pub class_name: String,
    pub enabled: bool,
    /// Class default with the instance's override applied, normalized to the
    /// component's serialized shape, plus `__slot_id` (and `__transform` for
    /// child components) metadata.
    pub data: Value,
}

/// A component placed on its own child object.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedChild {
    pub component: PlannedComponent,
    /// Slot of the child this one nests under; `None` = the root.
    pub parent_slot: Option<String>,
    pub local: LocalTransform,
}

/// The instantiation of one class instance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstancePlan {
    pub root: Vec<PlannedComponent>,
    pub children: Vec<PlannedChild>,
    /// Slots the instance removed (override `{"__removed": true}`).
    pub removed: Vec<String>,
}

impl InstancePlan {
    /// Default data of every slot (without the instance's overrides), as
    /// the override diff compares against.
    pub fn slot_ids(&self) -> impl Iterator<Item = &str> {
        self.root
            .iter()
            .map(|c| c.slot_id.as_str())
            .chain(self.children.iter().map(|c| c.component.slot_id.as_str()))
    }

    pub fn component(&self, slot_id: &str) -> Option<&PlannedComponent> {
        self.root.iter().find(|c| c.slot_id == slot_id).or_else(|| {
            self.children
                .iter()
                .map(|c| &c.component)
                .find(|c| c.slot_id == slot_id)
        })
    }
}

/// Whether a slot override marks the slot as removed.
pub fn is_removed_override(value: &Value) -> bool {
    value.get(REMOVED_KEY).and_then(Value::as_bool) == Some(true)
}

/// Slot defaults of `def` (no overrides applied), normalized, without metadata.
pub fn slot_default(def: &ClassDefinition, slot_id: &str) -> Option<Value> {
    let component = def.prefab.slot(slot_id)?;
    let normalized = normalize_component_data(&component.class_name, &component.data);
    Some(split_meta(&normalized).1)
}

/// Plan the components of `instance` from the current class definition.
pub fn plan_instance(def: &ClassDefinition, instance: &ClassInstance) -> InstancePlan {
    let mut plan = InstancePlan::default();
    let mut root_classes: HashSet<String> = HashSet::new();
    // Slot of the child entity each prefab index became, if it did.
    let mut child_slot_of_index: Vec<Option<String>> =
        Vec::with_capacity(def.prefab.components.len());

    for component in &def.prefab.components {
        let slot_id = component.slot_id.clone();
        let override_value = instance.component_overrides.get(&slot_id);
        if override_value.is_some_and(is_removed_override) {
            plan.removed.push(slot_id);
            child_slot_of_index.push(None);
            continue;
        }

        let (meta, _) = split_meta(&component.data);
        let normalized = normalize_component_data(&component.class_name, &component.data);
        let (_, mut data) = split_meta(&normalized);
        if let Some(patch) = override_value {
            apply(&mut data, patch);
        }

        let transform = meta
            .get(TRANSFORM_KEY)
            .map(LocalTransform::from_value)
            .or_else(|| {
                override_value
                    .and_then(|o| o.get(TRANSFORM_KEY))
                    .map(LocalTransform::from_value)
            });
        let parent_index = meta
            .get(PARENT_INDEX_KEY)
            .and_then(Value::as_u64)
            .map(|p| p as usize);
        let parent_child_slot = parent_index
            .and_then(|p| child_slot_of_index.get(p).cloned())
            .flatten();

        let is_child = transform.is_some()
            || parent_child_slot.is_some()
            || root_classes.contains(&component.class_name);

        if let Some(map) = data.as_object_mut() {
            map.insert(SLOT_ID_KEY.into(), Value::String(slot_id.clone()));
        }

        if is_child {
            let local = transform.unwrap_or_default();
            if let Some(map) = data.as_object_mut() {
                map.insert(TRANSFORM_KEY.into(), local.to_value());
            }
            plan.children.push(PlannedChild {
                component: PlannedComponent {
                    slot_id: slot_id.clone(),
                    class_name: component.class_name.clone(),
                    enabled: component.enabled,
                    data,
                },
                parent_slot: parent_child_slot,
                local,
            });
            child_slot_of_index.push(Some(slot_id));
        } else {
            root_classes.insert(component.class_name.clone());
            plan.root.push(PlannedComponent {
                slot_id,
                class_name: component.class_name.clone(),
                enabled: component.enabled,
                data,
            });
            child_slot_of_index.push(None);
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefab::{PrefabAsset, PrefabComponent};

    fn comp(class: &str, data: Value) -> PrefabComponent {
        PrefabComponent {
            slot_id: String::new(),
            class_name: class.into(),
            enabled: true,
            data,
        }
    }

    fn def(components: Vec<PrefabComponent>) -> ClassDefinition {
        let mut prefab = PrefabAsset {
            components,
            ..Default::default()
        };
        prefab.fill_missing_slot_ids();
        ClassDefinition {
            name: "Test".into(),
            prefab,
            ..Default::default()
        }
    }

    #[test]
    fn duplicates_and_transformed_components_become_children() {
        let def = def(vec![
            comp("A", json!({"v": 1})),
            comp("B", json!({"v": 2})),
            comp("A", json!({"v": 3})),
            comp(
                "C",
                json!({"v": 4, "__transform": {"position": [1.0, 0.0, 0.0]}}),
            ),
            comp("D", json!({"v": 5, "__parent_index": 3})),
        ]);
        let plan = plan_instance(&def, &ClassInstance::default());
        let root: Vec<_> = plan.root.iter().map(|c| c.slot_id.as_str()).collect();
        assert_eq!(root, ["A_0", "B_0"]);
        let children: Vec<_> = plan
            .children
            .iter()
            .map(|c| (c.component.slot_id.as_str(), c.parent_slot.as_deref()))
            .collect();
        assert_eq!(
            children,
            [("A_1", None), ("C_0", None), ("D_0", Some("C_0"))]
        );
        assert_eq!(plan.children[1].local.position, [1.0, 0.0, 0.0]);
        assert_eq!(plan.root[0].data["__slot_id"], "A_0");
    }

    #[test]
    fn overrides_apply_and_removed_slots_are_skipped() {
        let def = def(vec![
            comp("A", json!({"v": 1, "w": 2})),
            comp("B", json!({"v": 2})),
        ]);
        let mut instance = ClassInstance::default();
        instance
            .component_overrides
            .insert("A_0".into(), json!({"w": 9}));
        instance
            .component_overrides
            .insert("B_0".into(), json!({"__removed": true}));
        let plan = plan_instance(&def, &instance);
        assert_eq!(plan.root.len(), 1);
        assert_eq!(
            plan.root[0].data,
            json!({"v": 1, "w": 9, "__slot_id": "A_0"})
        );
        assert_eq!(plan.removed, ["B_0"]);
    }
}
