use engine_class_derive::register_scene_props_applier;
use pulsar_reflection::ScenePropsProjector;
use serde_json::Value;
use std::collections::HashMap;

use super::ProceduralTerrainComponent;

#[register_scene_props_applier]
impl ScenePropsProjector for ProceduralTerrainComponent {
    const CLASS_NAME: &'static str = "ProceduralTerrainComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        for key in [
            "enabled", "script_path", "script_function", "seed",
            "voxel_size", "chunk_size", "render_distance",
            "noise_type", "octaves", "lacunarity", "persistence",
            "base_height", "amplitude", "height_offset",
            "position", "rotation", "world_size",
            "base_color", "roughness", "metallic", "material_override",
            "layer_count", "blend_threshold",
            "meshing_algorithm", "enable_lod", "lod_levels", "enable_collision",
            "cast_shadows", "receive_shadows", "wireframe_overlay",
        ] {
            props.remove(key);
        }

        let Some(data) = component_data else { return; };

        let terrain = ProceduralTerrainComponent::from_component_data(data);
        for (k, v) in terrain.to_scene_props() {
            props.insert(k, v);
        }
    }
}
