//! Every authored slot of a skeletal mesh and of its animator has a property
//! editor, so neither the level editor nor the Blueprint component details
//! fall back to "(nyi)" for them.

use pulsar_reflection::REGISTRY;
use ui_common::PROPERTY_EDITOR_REGISTRY;

/// `class`'s `expected` properties exist and each has a property editor.
fn has_editors(class: &str, expected: &[&str]) {
    let instance = REGISTRY
        .create_instance(class)
        .unwrap_or_else(|| panic!("{class} is registered"));
    let props = instance.get_properties();
    let names: Vec<&str> = props.iter().map(|p| p.name).collect();
    for expected in expected {
        assert!(
            names.contains(expected),
            "{expected} is an inspector property of {class}: {names:?}"
        );
    }
    for prop in &props {
        if expected.contains(&prop.name) {
            assert!(
                PROPERTY_EDITOR_REGISTRY.has(prop.type_info.type_id),
                "`{}` ({}) has no property editor",
                prop.name,
                prop.type_info.type_name
            );
        }
    }
}

#[test]
fn skeletal_mesh_and_animator_properties_all_have_editors() {
    // linking the meshes and animation plugins registers the classes
    let _ = installed_plugins::meshes_component::SkeletalMeshComponent::default();
    let _ = installed_plugins::animation_component::AnimationComponent::default();
    has_editors("SkeletalMeshComponent", &["mesh"]);
    has_editors(
        "AnimationComponent",
        &[
            "animation_graph",
            "root_motion_mode",
            "root_motion_rotation",
            "root_motion_vertical",
            "root_motion_authority",
        ],
    );
}

#[test]
fn asset_ref_properties_round_trip_through_the_runtime_registry() {
    use pulsar_reflection::{AnimationGraphAssetKind, AssetRef, RUNTIME_TYPE_REGISTRY};
    let _ = installed_plugins::animation_component::AnimationComponent::default();
    let instance = REGISTRY.create_instance("AnimationComponent").unwrap();
    let props = instance.get_properties();
    let prop = props.iter().find(|p| p.name == "animation_graph").unwrap();
    let id = "33333333-3333-4333-8333-333333333333";
    let value = RUNTIME_TYPE_REGISTRY
        .deserialize_json_for_type(prop.type_info, serde_json::json!(id))
        .expect("a bare uuid string decodes into the Option property");
    let decoded = value
        .downcast_ref::<Option<AssetRef<AnimationGraphAssetKind>>>()
        .expect("the property decodes to Option<AssetRef<AnimationGraphAssetKind>>");
    assert_eq!(decoded.as_ref().map(AssetRef::as_str), Some(id));
    let json = RUNTIME_TYPE_REGISTRY
        .serialize_json_for_any(value.as_ref())
        .unwrap();
    assert_eq!(json, serde_json::json!(id));
    let none = RUNTIME_TYPE_REGISTRY
        .deserialize_json_for_type(prop.type_info, serde_json::Value::Null)
        .map(|v| {
            v.downcast_ref::<Option<AssetRef<AnimationGraphAssetKind>>>()
                .cloned()
        });
    assert!(
        matches!(none, Ok(Some(None))),
        "null clears the reference: {none:?}"
    );
}
