//! The meshes' record migrations, as a level load runs them
//! (`pulsar_class::records::migrate_component_records`).

use meshes_component::records::{FOLDED, FROM_PROPS};
use pulsar_class::records::migrate_component_records;
use serde_json::json;

#[test]
fn material_overrides_fold_into_the_mesh() {
    let mut root = json!({
        "objects": [{ "id": "a", "props": {}, "component_instances": [
            { "class_name": "StaticMeshComponent", "data": { "mesh_asset": "m.glb" } },
            { "class_name": "MaterialOverrideComponent", "data": { "roughness": 0.3 } }
        ]}]
    });
    let report = migrate_component_records(&mut root);
    assert_eq!(report.plugins, [("a".to_string(), FOLDED)]);
    let list = root["objects"][0]["component_instances"]
        .as_array()
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        list[0]["data"]["legacy_material_override"],
        json!({ "roughness": 0.3 })
    );
}

#[test]
fn a_bare_mesh_asset_prop_becomes_a_component_and_leaves_props() {
    let mut root = json!({
        "objects": [
            { "id": "a", "props": { "mesh_asset": "m.glb", "icon_asset": "i.png" } },
            { "id": "b", "props": { "mesh_asset": "m.glb" }, "component_instances": [] }
        ]
    });
    let report = migrate_component_records(&mut root);
    assert_eq!(report.plugins, [("a".to_string(), FROM_PROPS)]);
    assert_eq!(
        root["objects"][0]["component_instances"][0]["data"]["mesh_asset"],
        "m.glb"
    );
    assert_eq!(
        root["objects"][0]["props"],
        json!({ "icon_asset": "i.png" })
    );
    // An object with a component list keeps it as it is; the stale
    // copy goes.
    assert_eq!(root["objects"][1]["component_instances"], json!([]));
    assert_eq!(root["objects"][1]["props"], json!({}));
}
