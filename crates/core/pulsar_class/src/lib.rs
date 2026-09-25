//! Class assets and placed class instances (Pulsar-Native#921).
//!
//! A *class* is a directory under `<project>/src/classes/<Name>/` holding the
//! class graph, its compiled module and its `prefab.json` (the components
//! every instance gets). This crate gives classes a stable identity and turns
//! a placed class into real scene state:
//!
//! | Module | What |
//! |--------|------|
//! | [`id`] | [`ClassId`], the class GUID, kept in the class dir's `class.json` |
//! | [`prefab`] | the `prefab.json` read model, with a stable slot id per component |
//! | [`registry`] | [`ClassRegistry`]: GUID → class dir, by scanning `src/classes/*` |
//! | [`component`] | the [`ClassInstance`] world component a placed class carries |
//! | [`overrides`] | JSON diff / merge used for per-instance overrides |
//! | [`plan`] | which prefab component goes on the root and which on a child |
//! | [`world`] | [`world::instantiate_class`] and friends, on a SceneDB `World` |
//! | [`migrate`] | level-file migration from `ScriptComponent`/`blueprint_bindings` |
//! | [`natives`] | `class::slot_entity`, the script native slot-id lookups call |
//!
//! Placed instances *reference* their class: a level stores the class GUID
//! plus only the values that differ from the class defaults, and loading
//! rebuilds the instance from the current class definition.
//!
//! This crate is GPUI-free so the editor, the game runtime and command-line
//! tools can all use it.

pub mod component;
pub mod id;
pub mod migrate;
pub mod natives;
pub mod overrides;
pub mod plan;
pub mod prefab;
pub mod registry;
pub mod world;

pub use component::ClassInstance;
pub use id::{ClassId, ClassMeta, CLASS_META_FILE};
pub use plan::{plan_instance, InstancePlan, LocalTransform, PlannedChild, PlannedComponent};
pub use prefab::{BlueprintClassRef, PrefabAsset, PrefabComponent, PREFAB_FILE};
pub use registry::{ClassDefinition, ClassEntry, ClassRegistry};

/// Component class name of [`ClassInstance`].
pub const CLASS_INSTANCE: &str = "ClassInstance";

/// Metadata key a component's JSON carries when it was created from a class
/// prefab slot. `__`-prefixed keys are attachment metadata in the editor, so
/// it rides along with the component record and is never hydrated.
pub const SLOT_ID_KEY: &str = "__slot_id";

/// Metadata key for a prefab component's own local transform
/// (`{ "position": [..], "rotation": [..], "scale": [..] }`). A component that
/// carries one is placed on its own child entity.
pub const TRANSFORM_KEY: &str = "__transform";

/// Metadata key of the prefab's component hierarchy (index of the parent
/// component in `prefab.json`).
pub const PARENT_INDEX_KEY: &str = "__parent_index";

/// Override marker for a class slot the instance removed.
pub const REMOVED_KEY: &str = "__removed";

/// Stable id of the child object created for `slot_id` under the instance
/// root `root_id`.
pub fn child_stable_id(root_id: &str, slot_id: &str) -> String {
    format!("{root_id}#{slot_id}")
}
