//! Rust-export drift guard (#652).
//!
//! The Blueprint editor's Rust export (`pulsar_script_codegen::actor`) emits
//! the `impl Actor` block every generated game project compiles, but the
//! `pulsar_scenedb::Actor` trait it must satisfy is pinned by rev from the
//! root manifest, which the generator cannot see: drift between them would
//! surface only as E0053 inside user projects. The guard has three layers:
//!
//! 1. **Generated probe** ([`crate::export_probe`]): `build.rs` generates an
//!    actor through the real generator and the test build compiles it
//!    against the real pinned trait, drives it through the real
//!    [`crate::tick::TickLoop`], and asserts its signatures are
//!    byte-identical to the constants below.
//! 2. **Reference actor** ([`reference_actor`]): a hand-written twin of the
//!    emitted shape, so a pin bump that changes `Actor` stops compiling here
//!    and forces the generator and this twin to move together.
//! 3. **End-to-end check** (`tests/generated_project_compiles.rs`,
//!    `#[ignore]`, run via `just ci-drift-check`) generates a FULL project
//!    and `cargo check`s it against current pins.
//!
//! NOTE: the constants and the reference actor live side by side on purpose:
//! when one changes, change both in the same commit.

/// The exact `tick` signature the pinned, deliberately time-free
/// `pulsar_scenedb::Actor` expects, as the exporter must emit it.
pub(crate) const REFERENCE_TICK_SIGNATURE: &str =
    "fn tick(&mut self, _entity: Entity, _world: &mut World)";

/// The exact `begin_play` signature the exporter must emit.
pub(crate) const REFERENCE_BEGIN_PLAY_SIGNATURE: &str =
    "fn begin_play(&mut self, _entity: Entity, _world: &mut World)";

// ── Reference actor ──────────────────────────────────────────────────────────

pub(crate) mod reference_actor {
    // Same crates the emitted file names; if these stop resolving against
    // the pins, generated projects break the same way.
    use crate::prelude::*;
    use engine_class_derive::EngineClass;

    /// Mirrors the minimal component-less emission (`pub struct {Ty} {{}}`).
    #[derive(EngineClass, Clone)]
    pub struct DriftProbeReference {}

    // Deliberately hand-written: PBGC emits exactly this Default shape (the
    // EngineClass derive requires a `Default` constructor), and this twin
    // must mirror the emission, not be idiomatic.
    #[allow(clippy::derivable_impls)]
    impl Default for DriftProbeReference {
        fn default() -> Self {
            Self {}
        }
    }

    impl Actor for DriftProbeReference {
        // MUST stay byte-identical to REFERENCE_*_SIGNATURE above — the
        // emission assertions compare generator output against these lines.
        fn begin_play(&mut self, _entity: Entity, _world: &mut World) {}

        // Deliberately time-free, matching the pinned trait's contract (see
        // pulsar_scenedb::Actor's doc): frame timing flows through ECS
        // systems and blueprint dispatch, never through this callback.
        fn tick(&mut self, _entity: Entity, _world: &mut World) {}
    }

    /// Proves the mirrored shape is behaviorally correct against the pinned
    /// registry/tick loop, not merely compilable. Lifecycle ordering itself
    /// is covered by `tests.rs`'s `lifecycle_order`.
    #[test]
    fn reference_shape_registers_and_ticks_through_the_real_tick_loop() {
        let mut game = crate::tick::TickLoop::new(pulsar_core::TickMode::default(), 0);
        {
            let mut store = game.scene_store.write();
            game.actors
                .register(DriftProbeReference::default(), &mut store.world);
        }
        // Must complete without panicking.
        game.tick_once();
    }
}
