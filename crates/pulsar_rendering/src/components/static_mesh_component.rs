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
// Mat4/Quat/Vec3 used to build the transform passed to sync_mesh_object.

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

/// Attaches a mesh asset to a scene object.
#[derive(EngineClass, RegisterRuntimeBehavior, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Rendering")]
pub struct StaticMeshComponent {
    /// Relative asset path to the mesh file (e.g. "meshes/primitives/SM_Cube.fbx").
    ///
    /// Typed as [`MeshAssetPath`] so the property inspector renders a mesh-asset
    /// search browser instead of a plain text input.
    #[property]
    pub mesh_asset: MeshAssetPath,
}

impl ScenePropsProjector for StaticMeshComponent {
    const CLASS_NAME: &'static str = "StaticMeshComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        props.remove("mesh_asset");
        let Some(data) = component_data else { return };
        if let Some(path) = data
            .as_object()
            .and_then(|o| o.get("mesh_asset"))
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

        // Resolve to an absolute path so the context can use it as a stable
        // cache key regardless of which working directory the process uses.
        let abs_path = {
            let p = std::path::Path::new(&mesh_asset);
            if p.is_absolute() {
                p.to_string_lossy().replace('\\', "/")
            } else {
                context.project_root()
                    .join(&mesh_asset)
                    .to_string_lossy()
                    .replace('\\', "/")
            }
        };

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

        // Hand off to the context — it decides fast (transform update) or
        // slow (first-time disk load + GPU upload) based on its own caches.
        context.sync_mesh_object(
            scene_id_to_tag(owner.scene_object_id),
            &abs_path,
            transform,
            [pos.x, pos.y, pos.z, radius.max(0.1)],
        );
    }
}

