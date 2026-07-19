use engine_class_derive::register_scene_props_applier;
use pulsar_reflection::ScenePropsProjector;
use serde_json::Value;
use std::collections::HashMap;

use super::TerrainComponent;

#[register_scene_props_applier]
impl ScenePropsProjector for TerrainComponent {
    const CLASS_NAME: &'static str = "TerrainComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        for key in [
            "enabled", "voxel_data_source", "voxel_asset", "voxel_size", "chunk_size",
            "render_distance", "position", "rotation", "world_size",
            "voxel_material_source", "palette_texture", "base_color", "roughness", "metallic",
            "meshing_algorithm", "enable_lod", "lod_levels", "enable_collision",
            "cast_shadows", "receive_shadows", "wireframe_overlay",
        ] {
            props.remove(key);
        }

        let Some(data) = component_data else { return; };

        let terrain = TerrainComponent::from_component_data(data);
        for (k, v) in terrain.to_scene_props() {
            props.insert(k, v);
        }
    }
}
