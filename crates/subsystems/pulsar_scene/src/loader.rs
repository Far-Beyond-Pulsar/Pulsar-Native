//! Scene file helpers shared with `engine_backend`: component instance
//! records from a scene object's props, the legacy material-override
//! migration, and transform composition.
//!
//! The legacy import adapter that dispatched records through
//! `ComponentRuntimeBehavior::sync_component` straight into a Helio scene
//! is gone (Pulsar-Native#1035, Phase 4): every scene is hydrated into
//! SceneDB, and components reach the renderer through their data.
//!
//! ## Linker note
//!
//! `helio_component` types are re-exported below to create a live code
//! reference. Without it the linker can silently drop `helio_component`'s
//! `#[used]` inventory statics (its world-component and GPU registrations).

use std::collections::HashMap;

use glam::{EulerRot, Mat4, Quat, Vec3};
use serde_json::Value;

// ── Force helio_component into the binary ────────────────────────────────────
// Re-exporting these types creates a live symbol reference that prevents the
// linker from dropping helio_component's #[used] inventory statics.
pub use helio_component::FoliageComponent as _ForceLink_FoliageComponent;
pub use helio_component::LightComponent as _ForceLink_LightComponent;
pub use helio_component::PortalComponent as _ForceLink_PortalComponent;
pub use helio_component::PostProcessVolumeComponent as _ForceLink_PostProcessVolumeComponent;
pub use helio_component::ReflectionCaptureComponent as _ForceLink_ReflectionCaptureComponent;
pub use helio_component::StaticMeshComponent as _ForceLink_StaticMeshComponent;
pub use helio_component::WaterVolumeComponent as _ForceLink_WaterVolumeComponent;

// ── Shared public API (called by engine_backend) ─────────────────────────

/// Extract `(index, class_name, data)` from a component-instances value.
///
/// Prefers the explicit `component_instances` parameter (the modern path).
/// Falls back to `props["__component_instances"]` for backward compatibility
/// with v1/v2 scene files that embed this data inside the props map.
pub fn component_instances_from_props(
    props: &HashMap<String, Value>,
    component_instances: Option<&Value>,
) -> Vec<(usize, String, Value)> {
    let arr = component_instances.and_then(|v| v.as_array()).or_else(|| {
        props
            .get("__component_instances")
            .and_then(|v| v.as_array())
    });
    let Some(arr) = arr else {
        return Vec::new();
    };
    let mut records: Vec<_> = arr
        .iter()
        .enumerate()
        .filter_map(|(fi, entry)| {
            let o = entry.as_object()?;
            let idx = o
                .get("index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(fi);
            let cls = o
                .get("class_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)?;
            let dat = o.get("data").cloned().unwrap_or(Value::Null);
            Some((idx, cls, dat))
        })
        .collect();
    migrate_legacy_material_override_records(&mut records);
    records
}

/// Fold the retired single-material override into StaticMeshComponent's
/// hidden one-load migration field. This keeps old scene archives readable
/// while ensuring newly saved components use only mesh-owned material slots.
pub fn migrate_legacy_material_override_records(records: &mut Vec<(usize, String, Value)>) {
    let Some(override_index) = records
        .iter()
        .position(|(_, class_name, _)| class_name == "MaterialOverrideComponent")
    else {
        return;
    };
    let legacy_data = records[override_index].2.clone();
    let Some((_, _, mesh_data)) = records
        .iter_mut()
        .find(|(_, class_name, _)| class_name == "StaticMeshComponent")
    else {
        records.remove(override_index);
        return;
    };
    let Some(mesh_object) = mesh_data.as_object_mut() else {
        records.remove(override_index);
        return;
    };
    mesh_object
        .entry("legacy_material_override")
        .or_insert(legacy_data);
    records.remove(override_index);
}

/// Build transform from position / rotation (degrees YXZ) / scale.
/// Identical to engine's `build_transform`.
pub fn build_transform_parts(position: [f32; 3], rotation: [f32; 3], scale: [f32; 3]) -> Mat4 {
    let q = Quat::from_euler(
        EulerRot::YXZ,
        rotation[1].to_radians(),
        rotation[0].to_radians(),
        rotation[2].to_radians(),
    );
    Mat4::from_scale_rotation_translation(Vec3::from_array(scale), q, Vec3::from_array(position))
}
