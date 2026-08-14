use engine_class_derive::register_runtime_behavior;
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};

use super::PhysicsComponent;

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
