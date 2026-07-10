use engine_class_derive::register_runtime_behavior;
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};
use serde_json::Value;

use super::PhysicsComponent;

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for PhysicsComponent {
    const CLASS_NAME: &'static str = "PhysicsComponent";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        component_data: &Value,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
        let _physics = PhysicsComponent::from_component_data(component_data);
        // Runtime behavior: sync physics properties to the physics engine
        // This is a placeholder for actual physics engine integration
    }
}
