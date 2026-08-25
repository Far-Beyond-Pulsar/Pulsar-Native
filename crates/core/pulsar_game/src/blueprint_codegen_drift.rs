//! Blueprint-codegen drift guard (#652).
//!
//! PBGC (vendored at `crates/third-party/pbgc`) emits the `impl Actor` block
//! every generated game project compiles, but the `pulsar_scenedb::Actor`
//! trait it must satisfy is pinned by rev from the root manifest — the
//! vendored compiler cannot see that trait, so drift between them surfaces
//! only as E0053 inside user projects. This module is the guard:
//!
//! 1. **Reference actor** ([`reference_actor`]) — a hand-written twin of the
//!    emitted shape, compiled against the real pinned trait in THIS crate's
//!    test build and driven through the real [`crate::tick::TickLoop`]. If a
//!    pin bump changes `Actor`, this stops compiling and forces the
//!    generator and this twin to move together.
//! 2. **Exact-signature assertions** — PBGC runs through its public API at
//!    test time; the emitted source must contain byte-identical signature
//!    lines to the canonical constants below, and may not name any crate
//!    outside the pinned graph (`GameTime` in any spelling, or the phantom
//!    `gamma_core` event bus that once lived here).
//! 3. **End-to-end check** (`tests/generated_project_compiles.rs`,
//!    `#[ignore]`, run via `just ci-drift-check`) generates a FULL project
//!    and `cargo check`s it against current pins.
//!
//! A true always-compiled probe (build-script codegen + `include!`) was
//! attempted first and rejected: making pbgc a build-dependency of this
//! crate double-builds its `pulsar_std` chain as host and target units,
//! which collide on host==target platforms (Windows). The layered guard
//! above follows #652's sanctioned fallback instead.
//!
//! NOTE: the constants and the reference actor live side by side on purpose —
//! when one changes, change both in the same commit.

/// The exact `tick` signature the pinned, deliberately time-free
/// `pulsar_scenedb::Actor` expects, as PBGC must emit it.
pub(crate) const REFERENCE_TICK_SIGNATURE: &str =
    "fn tick(&mut self, _entity: Entity, _world: &mut World)";

/// The exact `begin_play` signature PBGC must emit.
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
                .register(DriftProbeReference::default(), store.world_mut());
        }
        // Must complete without panicking.
        game.tick_once();
    }
}

// ── Emission assertions ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use pbgc::{CompiledBlueprint, ProjectSpec};

    use super::{REFERENCE_BEGIN_PLAY_SIGNATURE, REFERENCE_TICK_SIGNATURE};

    /// Generate one class exactly the way the Blueprint Editor does.
    fn emit_minimal_actor() -> String {
        let bp = CompiledBlueprint::new(
            "drift_probe",
            r#"pub fn begin_play() {}

            pub fn tick() {}
            "#,
        )
        .with_tick(true)
        .with_begin_play(true);
        let spec = ProjectSpec::new("drift_probes").add_blueprint(bp);
        let project = pbgc::generate_project(&spec);
        project.files["src/classes/drift_probe/events/events.rs"].clone()
    }

    /// THE drift assertion (#652): PBGC's emitted `impl Actor` signatures
    /// must be byte-identical to the reference twin compiled against the
    /// pinned trait, and the emission may not name any crate the pinned
    /// graph does not provide.
    #[test]
    fn emitted_actor_signatures_match_the_pinned_actor_trait() {
        let actor = emit_minimal_actor();

        assert!(
            actor.contains(REFERENCE_TICK_SIGNATURE),
            "PBGC's emitted Actor::tick drifted from the pinned time-free \
             trait (expected `{REFERENCE_TICK_SIGNATURE}`)\n\nemitted:\n{actor}"
        );
        assert!(
            actor.contains(REFERENCE_BEGIN_PLAY_SIGNATURE),
            "PBGC's emitted Actor::begin_play drifted from the pinned trait"
        );
        assert!(
            !actor.contains("GameTime"),
            "`Actor::tick` is time-free at the pin; no GameTime may be emitted"
        );
        assert!(
            !actor.contains("gamma_core"),
            "emitted code names `gamma_core`, which does not exist in the \
             pinned graph (phantom bus removed by #652)"
        );
    }

    /// The component-bearing emission shape keeps compiling against current
    /// pins: Arc'd ComponentStore field, Default constructor, reflection
    /// imports, and the `__init_components` helper all typecheck textually
    /// here and for real in the end-to-end cargo check.
    #[test]
    fn component_bearing_emission_keeps_its_contracted_shape() {
        let bp = CompiledBlueprint::new("component_probe", "pub fn tick() {}").with_tick(true);
        let bp = bp.with_components(vec![pbgc::CompiledComponent {
            class_name: "RigidbodyComponent".to_string(),
            property_defaults: serde_json::json!({}),
            enabled: true,
        }]);
        let spec = ProjectSpec::new("drift_probes").add_blueprint(bp);
        let project = pbgc::generate_project(&spec);
        let actor = &project.files["src/classes/component_probe/events/events.rs"];

        assert!(actor.contains("pub struct ComponentProbe"));
        assert!(actor.contains("__init_components"));
        assert!(actor.contains("pulsar_game::ComponentStore"));
        assert!(!actor.contains("gamma_core"));
    }

    /// The class-tree layout other components rely on (the core project
    /// builder scans these directories; engine_main discovers bytecode under
    /// `events/.build/`). Layout drift would silently orphan classes.
    #[test]
    fn generated_layout_matches_the_project_builder_contract() {
        let bp = CompiledBlueprint::new("layout_probe", "pub fn tick() {}").with_tick(true);
        let spec = ProjectSpec::new("drift_probes").add_blueprint(bp);
        let project = pbgc::generate_project(&spec);
        let paths: Vec<&str> = project.file_paths().collect();

        for expected in [
            "src/classes/mod.rs",
            "src/classes/layout_probe/mod.rs",
            "src/classes/layout_probe/events/mod.rs",
            "src/classes/layout_probe/events/events.rs",
            "src/classes/layout_probe/vars/mod.rs",
        ] {
            assert!(paths.contains(&expected), "missing generated file {expected}");
        }
    }
}
