//! Static mesh component for mesh asset assignment.

use engine_class_derive::{EngineClass, RegisterRuntimeBehavior};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner, RuntimeMeshDesc,
    ScenePropsProjector,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use glam::{Mat4, Vec3};
use helio::Movability;

/// Static mesh assignment component.
///
/// Stores the mesh asset path and additional properties used by mesh scene objects.
#[derive(EngineClass, RegisterRuntimeBehavior, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Rendering")]
pub struct StaticMeshComponent {
    /// Relative asset path to the mesh file (for example: "meshes/primitives/SM_Cube.fbx").
    #[property]
    pub mesh_asset: String,

    /// Movability setting for the mesh (e.g., Static, Movable).
    #[property]
    #[serde(skip)]
    pub movability: Option<Movability>,

    /// Material ID associated with the mesh.
    #[property]
    pub material: Option<String>,

    /// Transform matrix for the mesh.
    #[property]
    pub transform: Option<Mat4>,

    /// Bounding box for the mesh.
    #[property]
    pub bounds: Option<[f32; 4]>,
}

impl ScenePropsProjector for StaticMeshComponent {
    const CLASS_NAME: &'static str = "StaticMeshComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        // Clear previously projected values to avoid stale data.
        props.remove("mesh_asset");
        props.remove("movability");
        props.remove("material");
        props.remove("transform");
        props.remove("bounds");

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

        if let Some(movability) = data
            .as_object()
            .and_then(|obj| obj.get("movability"))
            .and_then(|v| v.as_str())
        {
            props.insert("movability".to_string(), Value::from(movability));
        }

        if let Some(material) = data
            .as_object()
            .and_then(|obj| obj.get("material"))
            .and_then(|v| v.as_str())
        {
            props.insert("material".to_string(), Value::from(material));
        }

        if let Some(transform) = data
            .as_object()
            .and_then(|obj| obj.get("transform"))
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                if arr.len() == 16 {
                    let floats: [f32; 16] = arr.iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect::<Vec<_>>()
                        .try_into()
                        .ok()?;
                    Some(Mat4::from_cols_array(&floats))
                } else {
                    None
                }
            })
        {
            props.insert("transform".to_string(), Value::from(transform.to_cols_array()));
        }

        if let Some(bounds) = data
            .as_object()
            .and_then(|obj| obj.get("bounds"))
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                if arr.len() == 4 {
                    Some([
                        arr[0].as_f64().unwrap_or(0.0) as f32,
                        arr[1].as_f64().unwrap_or(0.0) as f32,
                        arr[2].as_f64().unwrap_or(0.0) as f32,
                        arr[3].as_f64().unwrap_or(0.0) as f32,
                    ])
                } else {
                    None
                }
            })
        {
            props.insert("bounds".to_string(), Value::from(bounds));
        }
    }
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::ScenePropsApplierRegistration {
        class_name: <StaticMeshComponent as ScenePropsProjector>::CLASS_NAME,
        apply: <StaticMeshComponent as ScenePropsProjector>::apply_scene_props,
    }
}

impl ComponentRuntimeBehavior for StaticMeshComponent {
    const CLASS_NAME: &'static str = "StaticMeshComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let mesh_asset = component_data
            .as_object()
            .and_then(|obj| obj.get("mesh_asset"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_string();

        if mesh_asset.is_empty() {
            context.report_error(format!(
                "StaticMeshComponent on '{}' has no mesh_asset",
                owner.scene_object_id
            ));
            return;
        }

        context.upsert_mesh(RuntimeMeshDesc {
            actor_key: format!("{}::mesh::{}", owner.scene_object_id, component_index),
            mesh_asset,
        });
    }
}

