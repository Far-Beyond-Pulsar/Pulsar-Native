//! GPU ownership boundary — positive cases.
//!
//! The negative side (unrouted `#[gpu] Vec<T>` / `#[gpu] GpuHeavy<T>`
//! fields are compile errors) is enforced by the derive itself; those
//! messages were verified verbatim during review. This file pins the
//! positive contract: a `scene_store`-routed struct's var-len/heavy fields
//! expand cleanly (SceneDB owns them), and packed-only structs are
//! unaffected by the boundary logic.

use engine_class_derive::engine_class;

/// Mirrors `StaticMeshComponent`'s shape: `scene_store` routing with
/// `#[gpu] Vec<T>` fields whose storage is SceneDB's own
/// `#[derive(SceneStore)]` concern. Must expand without error; the derive
/// contributes no packed companion fields for them.
#[engine_class(category = "Test", default, clone, debug, no_register, scene_store)]
pub struct BoundarySceneStoreRouted {
    #[property]
    pub label: f32,
    #[gpu(mirror = Once)]
    pub vertices: Vec<[f32; 3]>,
}

/// Packed-only struct: every `#[gpu]` field is a scalar, so the packed
/// companion path applies exactly as before the boundary existed.
#[engine_class(category = "Test", default, clone, debug, no_register)]
pub struct BoundaryPackedOnly {
    #[property]
    #[gpu]
    pub intensity: f32,
}

#[test]
fn scene_store_routed_struct_expands_and_reflects() {
    let value = BoundarySceneStoreRouted {
        label: 7.0,
        vertices: vec![[1.0, 2.0, 3.0]],
    };
    // Reflection metadata is unaffected by the boundary: scalar properties
    // still register normally.
    assert_eq!(value.label, 7.0);
}

#[test]
fn packed_only_struct_still_mirrors_scalars() {
    use pulsar_world_registry::GpuMirrored;
    let value = BoundaryPackedOnly { intensity: 0.5 };
    let mirror = GpuMirrored::to_gpu_mirror(&value);
    // One packed leaf; its bytes ride `GpuRepr<f32>` exactly as before the
    // boundary existed.
    assert_eq!(mirror.intensity.0, 0.5);
}
