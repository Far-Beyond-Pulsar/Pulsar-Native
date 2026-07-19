use engine_class_derive::register_runtime_behavior;
use pulsar_reflection::{
    get_subsystem, ComponentRuntimeBehavior, ComponentRuntimeContext, LiveKeySet,
    RuntimeComponentOwner,
};
use pulsar_rendering::subsystems::VoxelTerrainCache;
use serde_json::Value;

use super::TerrainComponent;

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for TerrainComponent {
    const CLASS_NAME: &'static str = "TerrainComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        _component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        get_subsystem!(context, LiveKeySet).insert(owner.scene_object_id.to_string());

        let cache = get_subsystem!(context, VoxelTerrainCache);
        let entry = cache.get_or_create(owner.scene_object_id);

        let asset_path = component_data
            .as_object()
            .and_then(|o| o.get("voxel_asset"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if !asset_path.is_empty() {
            // Voxel asset loading not yet implemented — use empty terrain as placeholder.
            if !entry.dirty {
                entry.grid = pulsar_rendering::subsystems::VoxelGrid::empty();
                entry.dirty = true;
            }
        } else {
            // No asset path — generate default procedural terrain with fixed seed.
            entry.sync_procedural(42, 28.8, 14.08, 1.0 / 18.0, 6, 2.0, 0.5, 0);
        }
    }
}
