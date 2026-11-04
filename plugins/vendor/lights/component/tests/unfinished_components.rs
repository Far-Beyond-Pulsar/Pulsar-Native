//! Components this engine keeps but does not consume yet say so: each
//! declares a reason and its tracking issue, which the properties card shows
//! and the first attach logs.

// Link the components into this test binary.
#[allow(unused_imports)]
use helio_component::components::StaticMeshComponent as _;
#[allow(unused_imports)]
use lights_component::ReflectionCaptureComponent as _;

#[test]
fn unfinished_components_declare_why_and_their_issue() {
    for (class, issue) in [("ReflectionCaptureComponent", 1054)] {
        let unfinished = pulsar_world_registry::unfinished_component(class)
            .unwrap_or_else(|| panic!("{class} is not declared unfinished"));
        assert!(!unfinished.reason.is_empty(), "{class} declares no reason");
        assert_eq!(
            unfinished.issue,
            format!("https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/{issue}"),
            "{class}"
        );
    }
    for class in ["StaticMeshComponent", "LODComponent", "LightComponent"] {
        assert!(
            pulsar_world_registry::unfinished_component(class).is_none(),
            "{class}"
        );
    }
}
