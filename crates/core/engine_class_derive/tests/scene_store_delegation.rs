//! Phase A verification (see `.claude/plans/eager-plotting-lecun.md`,
//! "Phase A — Retarget `engine_class_derive`'s SceneDB codegen to
//! `World`/`Entity`"): proves `#[engine_class(scene_store, ...)]` delegates
//! to `pulsar_scenedb::SceneStore` correctly on a throwaway struct, and that
//! the reflection half (`#[property]`/`EngineClass::get_properties`) still
//! works on the very same struct, side by side with SceneDB storage -- the
//! two were never in tension, but this is the first struct that actually
//! exercises both derived facets at once, proving the delegation didn't
//! silently break either.
//!
//! SCOPE NOTE -- what this does NOT prove: the `gpu` feature is
//! deliberately off for this test (see this crate's Cargo.toml
//! `[dev-dependencies]` comment on `pulsar_scenedb` for why -- a
//! pre-existing, unrelated version skew between this workspace's pinned
//! Pulsar-Reflection rev and what `pulsar_scenedb`'s own `gpu`-gated
//! modules require). So this proves the derive delegates correctly and
//! produces a valid `World`-storable, reflection-visible type (`Pod`, plain
//! `World::insert`/`get`, `EngineClass::get_properties`), but NOT that a
//! `#[gpu(...)]` field on an `#[engine_class(scene_store)]` struct actually
//! lands in a GPU buffer -- `pulsar_scenedb`'s own `tests/world_gpu_mirror.rs`
//! already proves that mechanism for a plain `#[derive(SceneStore)]` struct;
//! what's untested here is specifically the *delegation path*, under `gpu`.
//! Re-run this file with the feature on once the version skew above is
//! resolved to close that gap.

use engine_class_derive::engine_class;
use pulsar_reflection::EngineClass as _;
use pulsar_scenedb::World;

/// The struct under test: a real `#[engine_class]` component (reflection-
/// visible `label` field, exactly like every other component in
/// `pulsar_rendering`) that ALSO opts into SceneDB storage via the new
/// `scene_store` flag, with one `#[gpu]` field -- present to prove a
/// `#[gpu]`-annotated field still compiles and behaves as an ordinary field
/// with the `gpu` feature off (see this file's module doc), matching
/// `pulsar_scenedb`'s own "before `attach_gpu_mirror`, nothing special
/// happens" guarantee. `no_register` keeps this hermetic -- no global
/// `inventory` registration from a throwaway test type.
#[engine_class(category = "Test", default, clone, debug, no_register, scene_store)]
pub struct ThrowawayComponent {
    #[property]
    pub label: f32,
    #[gpu]
    pub mesh: u32,
}

#[test]
fn engine_class_scene_store_struct_round_trips_through_world() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, ThrowawayComponent { label: 7.0, mesh: 0xBEEF });

    let stored = world.get::<ThrowawayComponent>(entity).unwrap();
    assert_eq!(stored.label, 7.0);
    assert_eq!(stored.mesh, 0xBEEF);

    // Overwrite (update) must land too, not just first insert.
    world.insert(entity, ThrowawayComponent { label: 7.0, mesh: 0x1234 });
    assert_eq!(world.get::<ThrowawayComponent>(entity).unwrap().mesh, 0x1234);
}

#[test]
fn engine_class_scene_store_struct_is_still_reflection_visible() {
    // The whole point: SceneDB storage and the reflection/properties-panel
    // system read the SAME struct, not two parallel representations.
    let value = ThrowawayComponent { label: 42.0, mesh: 0 };
    let props = value.get_properties();
    assert!(
        props.iter().any(|p| p.name == "label"),
        "engine_class's #[property] metadata must survive scene_store delegation, got: {:?}",
        props.iter().map(|p| p.name).collect::<Vec<_>>()
    );
}
