use engine_class_derive::register_runtime_behavior;
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};
use serde_json::Value;

use super::ProceduralTerrainComponent;

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for ProceduralTerrainComponent {
    const CLASS_NAME: &'static str = "ProceduralTerrainComponent";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component_data: &Value,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
    }
}
