use engine_class_derive::register_runtime_behavior;
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};
use serde_json::Value;

use super::TerrainComponent;

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for TerrainComponent {
    const CLASS_NAME: &'static str = "TerrainComponent";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component_data: &Value,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
        // Stub: voxel terrain rendering integration with helio renderer will
        // be implemented once the rendering subsystem supports chunked voxel
        // meshes generated from the voxel data source.
    }
}
