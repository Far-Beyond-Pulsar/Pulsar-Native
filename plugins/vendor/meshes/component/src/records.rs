//! The meshes' legacy record migrations
//! (`pulsar_class::records::RecordMigrationRegistration`), run on every
//! level load with the engine's own.

use serde_json::{Map, Value};

const MATERIAL_OVERRIDE: &str = "MaterialOverrideComponent";
const STATIC_MESH: &str = "StaticMeshComponent";

/// What [`fold_material_override`] did, for reports.
pub const FOLDED: &str = "folded MaterialOverrideComponent into the mesh's material slots";
/// What [`mesh_from_props`] did, for reports.
pub const FROM_PROPS: &str = "turned props.mesh_asset into a StaticMeshComponent";

/// Folds the retired `MaterialOverrideComponent` into the object's mesh
/// (dropped with no mesh to take it). Returns whether the list held one.
fn fold_material_override(entries: &mut Vec<Value>) -> bool {
    let Some(position) = entries
        .iter()
        .position(|e| e.get("class_name").and_then(Value::as_str) == Some(MATERIAL_OVERRIDE))
    else {
        return false;
    };
    let legacy = entries
        .remove(position)
        .get("data")
        .cloned()
        .unwrap_or(Value::Null);
    let mesh = entries
        .iter_mut()
        .find(|e| e.get("class_name").and_then(Value::as_str) == Some(STATIC_MESH))
        .and_then(|e| e.get_mut("data"))
        .and_then(Value::as_object_mut);
    if let Some(mesh) = mesh {
        mesh.entry("legacy_material_override").or_insert(legacy);
    }
    true
}

/// A `StaticMeshComponent` entry for `object`'s non-empty
/// `props.mesh_asset`, in the class's own shape.
fn mesh_from_props(object: &Value) -> Option<Value> {
    let path = object
        .get("props")?
        .get("mesh_asset")?
        .as_str()
        .filter(|path| !path.trim().is_empty())?;
    let mut data = pulsar_reflection::REGISTRY
        .create_instance(STATIC_MESH)
        .and_then(|instance| instance.to_json().ok())
        .unwrap_or_else(|| Value::Object(Map::new()));
    data.as_object_mut()?
        .insert("mesh_asset".into(), Value::String(path.to_string()));
    Some(serde_json::json!({
        "class_name": STATIC_MESH,
        "enabled": true,
        "data": data,
    }))
}

inventory::submit! {
    pulsar_class::records::RecordMigrationRegistration {
        origin: pulsar_plugins::origin!(),
        description: FOLDED,
        entries: Some(fold_material_override),
        from_props: None,
        stale_props: &[],
    }
}

inventory::submit! {
    pulsar_class::records::RecordMigrationRegistration {
        origin: pulsar_plugins::origin!(),
        description: FROM_PROPS,
        entries: None,
        from_props: Some(mesh_from_props),
        stale_props: &["mesh_asset"],
    }
}
