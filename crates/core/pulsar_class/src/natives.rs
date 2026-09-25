//! Script natives for class instances.
//!
//! `class::slot_entity(root, slot_id)` is what a Blueprint
//! `get_component_ref` node with a slot id compiles to (followed by
//! `Class::of`): the entity holding that prefab slot's component, which is
//! the root itself or one of its generated children. When the slot is not
//! found (an instance built before slot ids, or a removed slot) it returns
//! `root`, so the lookup degrades to the old by-class behaviour.

use pulsar_scenedb::Entity;
use pulsar_script_vm::{Host, NativeFn, NativeRegistration};

/// Name of the slot lookup native.
pub const SLOT_ENTITY_NATIVE: &str = "class::slot_entity";

inventory::submit! {
    NativeRegistration {
        build: || {
            NativeFn::builder(SLOT_ENTITY_NATIVE)
                .doc("The entity holding a class prefab slot's component on the class instance `root`.")
                .side_effect_free()
                .attr("category", "Class")
                .params(["root", "slot_id"])
                .build(|host: &mut Host<'_>, root: Entity, slot_id: String| {
                    crate::world::slot_entity(host.world, root, &slot_id).unwrap_or(root)
                })
        },
    }
}
