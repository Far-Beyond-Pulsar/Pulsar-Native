//! Rust-export drift guard (#652).
//!
//! The Blueprint editor's Rust export (`pulsar_script_codegen::actor`) emits
//! the `impl pulsar_game::lifecycle::NativeActor` block every generated game
//! project compiles, but the generator cannot see that trait: drift between
//! them would surface only as E0053 inside user projects. The guard has three
//! layers:
//!
//! 1. **Generated probe** ([`crate::export_probe`]): `build.rs` generates an
//!    actor through the real generator and the test build compiles it
//!    against the real trait, drives it through the real
//!    [`crate::tick::TickLoop`], and asserts its signatures are
//!    byte-identical to the constants below.
//! 2. **Reference actor** ([`reference_actor`]): a hand-written twin of the
//!    emitted shape, so a change to `NativeActor` stops compiling here
//!    and forces the generator and this twin to move together.
//! 3. **End-to-end check** (`tests/generated_project_compiles.rs`,
//!    `#[ignore]`, run via `just ci-drift-check`) generates a FULL project
//!    and `cargo check`s it against current pins.
//!
//! NOTE: the constants and the reference actor live side by side on purpose:
//! when one changes, change both in the same commit.

/// The impl line the exporter must emit: the trait by its full path, so the
/// generated file does not depend on the prelude's `Actor` (SceneDB's, for the
/// deprecation window).
pub(crate) const REFERENCE_IMPL_LINE: &str = "impl pulsar_game::lifecycle::NativeActor for ";

/// The exact `tick` signature `NativeActor` expects, as the exporter must
/// emit it.
pub(crate) const REFERENCE_TICK_SIGNATURE: &str = "fn tick(&mut self, _entity: Entity, _world: &mut World, _cx: &mut pulsar_game::lifecycle::ActorCx<'_>, _delta_seconds: f32)";

/// The exact `begin_play` signature the exporter must emit.
pub(crate) const REFERENCE_BEGIN_PLAY_SIGNATURE: &str = "fn begin_play(&mut self, _entity: Entity, _world: &mut World, _cx: &mut pulsar_game::lifecycle::ActorCx<'_>)";

/// The exact `end_play` signature the exporter must emit.
pub(crate) const REFERENCE_END_PLAY_SIGNATURE: &str = "fn end_play(&mut self, _entity: Entity, _world: &mut World, _cx: &mut pulsar_game::lifecycle::ActorCx<'_>)";

// ── Reference actor ──────────────────────────────────────────────────────────

pub(crate) mod reference_actor {
    // Same crates the emitted file names; if these stop resolving against
    // the pins, generated projects break the same way.
    use crate::prelude::*;
    use engine_class_derive::EngineClass;

    /// A minimal stand-in for an exported actor: only its `impl NativeActor`
    /// signatures have to match what the exporter emits.
    #[derive(EngineClass, Clone)]
    pub struct DriftProbeReference {}

    // Deliberately hand-written: the EngineClass derive requires a `Default`
    // constructor, as the exporter's `impl Default` provides.
    #[allow(clippy::derivable_impls)]
    impl Default for DriftProbeReference {
        fn default() -> Self {
            Self {}
        }
    }

    impl pulsar_game::lifecycle::NativeActor for DriftProbeReference {
        // MUST stay byte-identical to REFERENCE_*_SIGNATURE above: the
        // emission assertions compare generator output against these lines.
        fn begin_play(&mut self, _entity: Entity, _world: &mut World, _cx: &mut pulsar_game::lifecycle::ActorCx<'_>) {}

        fn tick(&mut self, _entity: Entity, _world: &mut World, _cx: &mut pulsar_game::lifecycle::ActorCx<'_>, _delta_seconds: f32) {}

        fn end_play(&mut self, _entity: Entity, _world: &mut World, _cx: &mut pulsar_game::lifecycle::ActorCx<'_>) {}
    }

    /// Proves the mirrored shape is behaviorally correct against the real
    /// tick loop, not merely compilable. Lifecycle ordering itself
    /// is covered by `tests.rs`'s `lifecycle_order`.
    #[test]
    fn reference_shape_registers_and_ticks_through_the_real_tick_loop() {
        let mut game = crate::tick::TickLoop::new(pulsar_core::TickMode::default(), 0);
        game.register_native_actor(DriftProbeReference::default());
        // Must complete without panicking.
        game.tick_once();
    }
}
