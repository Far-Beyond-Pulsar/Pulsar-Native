use engine_class_derive::{register_runtime_behavior, register_world_component};
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};

use super::PhysicsComponent;

// Phase B5 (Pulsar-Native#556). sync_component is a no-op stub today (real
// physics-engine integration hasn't landed) -- migrating it onto World
// storage now is a free win: nothing real to port, and it's ready for when
// that integration does land.
#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for PhysicsComponent {
    const CLASS_NAME: &'static str = "PhysicsComponent";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component: &Self,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
        // Runtime behavior: sync physics properties to the physics engine
        // This is a placeholder for actual physics engine integration -- was
        // already a no-op (deserialized-then-discarded) before this typed
        // signature; `_component` is `&Self` now, nothing left to convert.
    }
}
