use engine_class_derive::{register_runtime_behavior, register_world_component};
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};

use super::RigidbodyComponent;

// Phase B5 (Pulsar-Native#556). Same free-win rationale as PhysicsComponent
// -- sync_component is a no-op stub today.
#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for RigidbodyComponent {
    const CLASS_NAME: &'static str = "RigidbodyComponent";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component: &Self,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
        // Runtime behavior: sync rigidbody properties to the physics engine
        // This is a placeholder for actual physics engine integration -- was
        // already a no-op (deserialized-then-discarded) before this typed
        // signature; `_component` is `&Self` now, nothing left to convert.
    }
}
