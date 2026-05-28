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

        // Resolve the asset path — used as the mesh-cache key so two objects
        // sharing the same FBX reuse the same GPU geometry.
        let abs_path = {
            let p = std::path::Path::new(&mesh_asset);
            if p.is_absolute() { p.to_path_buf() }
            else {
                context.project_root().join(&mesh_asset)
            }
        };
        let mesh_key = abs_path.to_string_lossy().replace('\\', "/");

        // Load geometry from disk (context caches after first load).
        let Some(upload) = context.load_mesh_file(&abs_path) else {
            context.report_error(format!(
                "StaticMeshComponent on '{}': failed to load mesh '{}'",
                owner.scene_object_id, mesh_asset
            ));
            return;
        };

        // Build world-space transform from owner — same every call for a
        // given object position/rotation/scale.
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
        let bounds = [pos.x, pos.y, pos.z, radius.max(0.1)];

        // Delegate the insert-vs-update decision to the context.
        // The context tracks which helio objects already exist (keyed by tag)
        // and decides whether to update the transform or upload geometry fresh.
        let tag = scene_id_to_tag(owner.scene_object_id);
        context.track_mesh_object(tag, upload, mesh_key, transform, bounds);
    }
}

