//! Static mesh component for mesh asset assignment.

use engine_class_derive::EngineClass;
use pulsar_reflection::ScenePropsProjector;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Static mesh assignment component.
///
/// Stores the mesh asset path used by mesh scene objects.
#[derive(EngineClass, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Rendering")]
pub struct StaticMeshComponent {
    /// Relative asset path to the mesh file (for example: "meshes/primitives/SM_Cube.fbx").
    #[property]
    pub mesh_asset: String,
}

impl ScenePropsProjector for StaticMeshComponent {
    const CLASS_NAME: &'static str = "StaticMeshComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        // Always clear previously projected value to avoid stale asset assignments.
        props.remove("mesh_asset");

        let Some(data) = component_data else {
            return;
        };

        if let Some(path) = data
            .as_object()
            .and_then(|obj| obj.get("mesh_asset"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            props.insert("mesh_asset".to_string(), Value::from(path));
        }
    }
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::ScenePropsApplierRegistration {
        class_name: <StaticMeshComponent as ScenePropsProjector>::CLASS_NAME,
        apply: <StaticMeshComponent as ScenePropsProjector>::apply_scene_props,
    }
}
