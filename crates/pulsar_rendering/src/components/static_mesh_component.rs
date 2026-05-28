//! Static mesh component for mesh asset assignment.

use engine_class_derive::{EngineClass, RegisterRuntimeBehavior};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner,
    ReflectError, ScenePropsProjector, scene_id_to_tag,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{GpuMaterial, GroupMask, Movability, ObjectDescriptor, SceneActor};

// ── MeshAssetPath ─────────────────────────────────────────────────────────────

/// Strongly-typed wrapper for mesh asset paths.
///
/// Using this as a field type causes the reflection property inspector to render
/// a mesh-asset search browser (via `MeshAssetPicker`) instead of a plain text box.
///
/// Serialises transparently as a JSON string so existing scene files require no
/// migration.
///
/// # Example
///
/// ```ignore
/// #[property]
/// pub mesh_asset: MeshAssetPath,
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeshAssetPath(pub String);

impl MeshAssetPath {
    /// Create a new `MeshAssetPath` from any string-like value.
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// Borrow the inner path string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` if the path is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for MeshAssetPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for MeshAssetPath {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for MeshAssetPath {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ── Reflection registration ───────────────────────────────────────────────────

fn serialize_mesh_asset_path_json(
    value: &MeshAssetPath,
) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(value.0))
}

fn deserialize_mesh_asset_path_json(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<MeshAssetPath> {
    value
        .as_str()
        .map(|s| MeshAssetPath(s.to_string()))
        .ok_or_else(|| ReflectError::TypeMismatch {
            expected: "MeshAssetPath",
            found: format!("{:?}", value),
        })
}

/// Register `MeshAssetPath` with the reflection system.
///
/// `structure = String` makes `type_info.is_string()` return `true`, which
/// enables the property inspector to detect this type and render the
/// mesh-asset browser UI.
#[pulsar_reflection::pulsar_type(
    primitive,
    structure = String,
    serialize_json_with = serialize_mesh_asset_path_json,
    deserialize_json_with = deserialize_mesh_asset_path_json
)]
#[allow(dead_code)]
type RegisteredMeshAssetPath = MeshAssetPath;

// ── StaticMeshComponent ───────────────────────────────────────────────────────

/// Static mesh assignment component.
///
/// Stores the mesh asset path and additional properties used by mesh scene objects.
#[derive(EngineClass, RegisterRuntimeBehavior, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Rendering")]
pub struct StaticMeshComponent {
    /// Relative asset path to the mesh file (for example: "meshes/primitives/SM_Cube.fbx").
    ///
    /// Typed as [`MeshAssetPath`] so the property inspector renders the mesh-asset
    /// search browser instead of a plain text input.
    #[property]
    pub mesh_asset: MeshAssetPath,

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

        let path = std::path::PathBuf::from(&mesh_asset);
        let Some(upload) = context.load_mesh_file(&path) else {
            context.report_error(format!(
                "StaticMeshComponent on '{}': failed to load mesh '{}'",
                owner.scene_object_id, mesh_asset
            ));
            return;
        };

        // Two-step helio insert: mesh geometry → object instance.
        // The component owns this logic completely; the context has no mesh knowledge.
        let mesh_id = match context.renderer_mut().scene_mut()
            .insert_actor(SceneActor::mesh(upload))
            .as_mesh()
        {
            Some(id) => id,
            None => {
                context.report_error(format!(
                    "StaticMeshComponent on '{}': mesh insert returned non-mesh handle",
                    owner.scene_object_id
                ));
                return;
            }
        };

        let mat = GpuMaterial {
            base_color: [0.6, 0.6, 0.65, 1.0],
            emissive: [0.0; 4],
            roughness_metallic: [0.7, 0.0, 1.5, 0.5],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal:     GpuMaterial::NO_TEXTURE,
            tex_roughness:  GpuMaterial::NO_TEXTURE,
            tex_emissive:   GpuMaterial::NO_TEXTURE,
            tex_occlusion:  GpuMaterial::NO_TEXTURE,
            workflow: 0, flags: 0, _pad: 0,
        };
        let mat_id = context.renderer_mut().scene_mut().insert_material(mat);

        let q = Quat::from_euler(
            EulerRot::YXZ,
            owner.rotation[1].to_radians(),
            owner.rotation[0].to_radians(),
            owner.rotation[2].to_radians(),
        );
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::from_array(owner.scale),
            q,
            Vec3::from_array(owner.position),
        );
        let pos    = transform.w_axis.truncate();
        let radius = Vec3::from_array(owner.scale).length() * 0.5;

        // Tag the actor with a hash of the SceneDb ID so the picker can
        // identify it without any external reverse-lookup map.
        context.renderer_mut().scene_mut()
            .insert_actor(SceneActor::object(ObjectDescriptor {
                mesh:       mesh_id,
                material:   mat_id,
                transform,
                bounds:     [pos.x, pos.y, pos.z, radius.max(0.1)],
                flags:      0,
                groups:     GroupMask::NONE,
                movability: Some(Movability::Movable),
                user_tag:   scene_id_to_tag(owner.scene_object_id),
            }));
    }
}

