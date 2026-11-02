//! Components this engine keeps but does not consume yet say so: each
//! declares a reason and its tracking issue, which the properties card shows
//! and the first attach logs.

// Link the remaining unfinished component into this test binary.
use helio_component::components::ReflectionCaptureComponent as _;

#[test]
fn unfinished_components_declare_why_and_their_issue() {
    for (class, issue) in [
        ("ReflectionCaptureComponent", 1054),
    ] {
        let unfinished = pulsar_world_registry::unfinished_component(class)
            .unwrap_or_else(|| panic!("{class} is not declared unfinished"));
        assert!(!unfinished.reason.is_empty(), "{class} declares no reason");
        assert_eq!(
            unfinished.issue,
            format!("https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/{issue}"),
            "{class}"
        );
    }
    for class in [
        "StaticMeshComponent",
        "LODComponent",
        "LightComponent",
        "GlobalFogComponent",
        "LocalFogVolumeComponent",
        "PostProcessVolumeComponent",
        "CameraPostProcessComponent",
        "CameraLensComponent",
        "WaterVolumeComponent",
        "FoliageComponent",
        "VoxelTerrainComponent",
    ] {
        assert!(
            pulsar_world_registry::unfinished_component(class).is_none(),
            "{class}"
        );
    }
}
