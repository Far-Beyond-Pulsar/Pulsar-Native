use engine_class_derive::register_runtime_behavior;
use pulsar_reflection::{
    get_subsystem, ComponentRuntimeBehavior, ComponentRuntimeContext, LiveKeySet,
    RuntimeComponentOwner,
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
        get_subsystem!(context, LiveKeySet).insert(owner.scene_object_id.to_string());

        tracing::info!(
            "[TERRAIN] sync_component called for scene_object_id='{}', data_keys={:?}",
            owner.scene_object_id,
            component_data.as_object().map(|o| o.keys().collect::<Vec<_>>())
        );
        let cache = match get_subsystem!(context, VoxelTerrainCache) {
            c => c,
        };
        let entry = cache.get_or_create(owner.scene_object_id);

        // Build a hash of all generation parameters so we can skip regeneration
        // when nothing changed.
        let obj = component_data.as_object();
        tracing::info!(
            "[TERRAIN] component_data keys: {:?}",
            obj.map(|o| o.keys().collect::<Vec<_>>())
        );
        let seed = obj
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

        let frequency = 1.0 / 18.0;

        let params_hash = u64::from(seed)
            .wrapping_mul(31)
            .wrapping_add(octaves * 7)
            .wrapping_add((lacunarity as u64) * 11)
            .wrapping_add((persistence as u64) * 13)
            .wrapping_add((base_height as u64) * 17)
            .wrapping_add((amplitude as u64) * 19);

        tracing::info!(
            "[TERRAIN] params: seed={}, octaves={}, lacunarity={}, persistence={}, base_height={}, amplitude={}",
            seed, octaves, lacunarity, persistence, base_height, amplitude
        );

        entry.sync_procedural(seed, base_height as f32, amplitude as f32, frequency, octaves as u32, lacunarity as f32, persistence as f32, params_hash);

        tracing::info!(
            "[TERRAIN] after sync_procedural: dirty={}, params_hash={}, solid_voxels={}",
            entry.dirty,
            entry.params_hash,
            entry.grid.materials.iter().filter(|&&m| m != 0).count()
        );
    }
}
