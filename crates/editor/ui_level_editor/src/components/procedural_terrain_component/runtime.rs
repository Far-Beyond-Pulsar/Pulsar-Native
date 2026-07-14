use engine_class_derive::register_runtime_behavior;
use pulsar_reflection::{
    get_subsystem, ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner,
};
use pulsar_rendering::subsystems::{TerrainEntry, VoxelTerrainCache};
use serde_json::Value;

use super::ProceduralTerrainComponent;

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for ProceduralTerrainComponent {
    const CLASS_NAME: &'static str = "ProceduralTerrainComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        _component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let cache = get_subsystem!(context, VoxelTerrainCache);
        let entry = cache.get_or_create(owner.scene_object_id);

        // Build a hash of all generation parameters so we can skip regeneration
        // when nothing changed.
        let seed = component_data
            .as_object()
            .and_then(|o| o.get("seed"))
            .and_then(|v| v.as_u64())
            .unwrap_or(42) as u32;

        let octaves = component_data
            .as_object()
            .and_then(|o| o.get("octaves"))
            .and_then(|v| v.as_u64())
            .unwrap_or(6);

        let lacunarity = component_data
            .as_object()
            .and_then(|o| o.get("lacunarity"))
            .and_then(|v| v.as_f64())
            .unwrap_or(2.0);

        let persistence = component_data
            .as_object()
            .and_then(|o| o.get("persistence"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5);

        let base_height = component_data
            .as_object()
            .and_then(|o| o.get("base_height"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let amplitude = component_data
            .as_object()
            .and_then(|o| o.get("amplitude"))
            .and_then(|v| v.as_f64())
            .unwrap_or(500.0);

        // Hash all params to detect changes
        let params_hash = u64::from(seed)
            .wrapping_mul(31)
            .wrapping_add(octaves * 7)
            .wrapping_add((lacunarity as u64) * 11)
            .wrapping_add((persistence as u64) * 13)
            .wrapping_add((base_height as u64) * 17)
            .wrapping_add((amplitude as u64) * 19);

        entry.sync_procedural(seed, params_hash);
    }
}
