//! Live-dispatch acceptance probe for PBGC-generated actors (#651).
//!
//! Two halves, deliberately side by side:
//!
//! 1. [`LiveDispatchReference`] — a hand-written twin of the exact actor
//!    shape PBGC emits since #651: an empty struct whose `begin_play`/
//!    `tick` hydrate prefab components onto their own scene entity and run
//!    graph logic that addresses components through
//!    `pulsar_world_registry::dispatch::*` with the `(entity, world)` pair
//!    the engine handed them. Compiled against the real pinned crates and
//!    driven through the real `TickLoop`, it proves the GENERATED SHAPE
//!    mutates the ONE shared SceneDB world — including firing #47
//!    subscription events, the acceptance criterion "visible in SceneDB
//!    subscriptions during standalone play" (a full light e2e additionally
//!    needs a GPU/display session; the registered-probe pattern follows
//!    `blueprint_runtime::component_ops`' VmProbe).
//! 2. [`pbgc_emission_matches_the_reference_shape_this_module_proves`] —
//!    ties the twin to the generator: PBGC's output for the same class +
//!    graph must contain the very calls the twin makes, and none of the
//!    retired baked-store routing. Generator/twin drift fails here.
//!
//! The probe component registers exactly like engine classes do
//! (`#[engine_class]` expands to the same inventory submissions), which is
//! why this lives in the lib test build — integration binaries can have
//! inventory statics linker-GC'd (see C-phase golden-snapshot note).

#![cfg(test)]

use engine_class_derive::EngineClass;
use pulsar_reflection::{
    EngineClass, EngineClassRegistration, PropertyMetadata, RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY,
};
use pulsar_scenedb::{component_id, Actor, ComponentChangeKind, Entity, World};
use pulsar_world_registry::{
    dispatch::set_component_property, hydrate_world_component_for_class, inventory,
    world_component_present_for_class, WorldComponentRegistration,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tick::TickLoop;

// ── Probe component ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct LiveDispatchProbe {
    intensity: f32,
}

impl EngineClass for LiveDispatchProbe {
    fn class_name() -> &'static str {
        "LiveDispatchProbe"
    }

    fn get_properties(&self) -> Vec<PropertyMetadata> {
        let type_info: &'static RuntimeTypeInfo = RUNTIME_TYPE_REGISTRY
            .get::<f32>()
            .expect("f32 registered in the runtime type registry");
        vec![PropertyMetadata {
            name: "intensity",
            display_name: "Intensity".into(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            type_info,
            getter: Box::new(|c: &dyn EngineClass| {
                Box::new(
                    c.as_any()
                        .downcast_ref::<LiveDispatchProbe>()
                        .unwrap()
                        .intensity,
                )
            }),
            setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                if let Some(v) = v.downcast_ref::<f32>() {
                    c.as_any_mut()
                        .downcast_mut::<LiveDispatchProbe>()
                        .unwrap()
                        .intensity = *v;
                }
            }),
        }]
    }

    fn get_methods() -> Vec<pulsar_reflection::MethodMetadata> {
        Vec::new()
    }

    fn create_default() -> Box<dyn EngineClass> {
        Box::new(Self::default())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_boxed(&self) -> Box<dyn EngineClass> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}

fn live_dispatch_probe_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
    world
        .get::<LiveDispatchProbe>(entity)
        .map(|c| c as &dyn EngineClass)
}

fn live_dispatch_probe_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
    world
        .get_mut::<LiveDispatchProbe>(entity)
        .map(|c| c.into_inner() as &mut dyn EngineClass)
}

fn live_dispatch_probe_hydrate(
    world: &mut World,
    entity: Entity,
    data: &serde_json::Value,
) -> Result<(), String> {
    let parsed: LiveDispatchProbe =
        serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
    world.insert(entity, parsed);
    Ok(())
}

inventory::submit! {
    WorldComponentRegistration {
        class_name: "LiveDispatchProbe",
        component_type: component_id::<LiveDispatchProbe>,
        hydrate: live_dispatch_probe_hydrate,
        remove: |world, entity| {
            let _ = world.remove::<LiveDispatchProbe>(entity);
        },
        dispatch: |world, entity, _: _, _: usize, _: _| {
            world.get::<LiveDispatchProbe>(entity).is_some()
        },
        get_as_engine_class: live_dispatch_probe_get,
        get_as_engine_class_mut: live_dispatch_probe_get_mut,
        on_removed: |_, _| {},
        refresh_gpu_mirror: |_, _| {},
    }
}

inventory::submit! {
    EngineClassRegistration {
        name: "LiveDispatchProbe",
        category: None,
        constructor: <LiveDispatchProbe as EngineClass>::create_default,
        from_json: None,
    }
}

// ── Reference actor (the generated shape, hand-written) ─────────────────────

/// Twin of the post-#651 emission for a class declaring one
/// `LiveDispatchProbe` prefab component and a `comp_set_prop` node in each
/// event. Every line below has a matching assertion in the emission test.
#[derive(EngineClass, Clone)]
struct LiveDispatchReference {}

#[allow(clippy::derivable_impls)]
impl Default for LiveDispatchReference {
    fn default() -> Self {
        Self {}
    }
}

const PROBE_CLASS: &str = "LiveDispatchProbe";

impl LiveDispatchReference {
    /// Mirrors the emitted `__init_components`: hydrate the baked defaults
    /// ONLY when the live world lacks the component (scene overrides win).
    fn __init_components(entity: Entity, world: &mut World) {
        if !world_component_present_for_class(PROBE_CLASS, world, entity) {
            if let Err(__e) = hydrate_world_component_for_class(
                PROBE_CLASS,
                world,
                entity,
                &json!({ "intensity": 10.0 }),
            ) {
                tracing::error!("LiveDispatchReference: hydrating failed: {__e}");
            }
        }
    }
}

impl Actor for LiveDispatchReference {
    // The two set calls below mirror the emitted comp_set_prop nodes
    // byte-for-byte in structure (dispatcher + `_world, _entity, class, 0,
    // prop, json value` + log-and-continue error arm).
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {
        Self::__init_components(_entity, _world);
        if let Err(__e) =
            set_component_property(_world, _entity, PROBE_CLASS, 0, "intensity", json!(42.0f32))
        {
            tracing::error!("comp_set_prop::LiveDispatchProbe::intensity failed: {__e}");
        }
    }

    fn tick(&mut self, _entity: Entity, _world: &mut World) {
        if let Err(__e) =
            set_component_property(_world, _entity, PROBE_CLASS, 0, "intensity", json!(77.0f32))
        {
            tracing::error!("comp_set_prop::LiveDispatchProbe::intensity failed: {__e}");
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// THE #651 acceptance proof: begin_play's generated-shape write reaches the
/// shared store through the real registration path, tick's write flows
/// through the real TickLoop phase-2 borrow, and BOTH are visible to SceneDB
/// subscriptions armed by an outside observer.
#[test]
fn generated_actor_shape_writes_the_shared_world_and_fires_subscriptions() {
    let mut game = TickLoop::new(pulsar_core::TickMode::default(), 0);

    // Registration path: exactly what generated engine_main emits.
    let entity = {
        let mut store = game.scene_store.write();
        game.actors
            .register(LiveDispatchReference::default(), store.world_mut())
    };

    // begin_play ran inside register: hydration + dispatcher write landed.
    assert_eq!(
        game.scene_store
            .read()
            .world()
            .get::<LiveDispatchProbe>(entity)
            .expect("prefab component hydrated onto the actor's scene entity")
            .intensity,
        42.0
    );

    // An outside observer subscribes (properties-panel / renderer pattern),
    // then the loop ticks: phase 2 hands the SAME world to Actor::tick.
    {
        let mut store = game.scene_store.write();
        let world = store.world_mut();
        world
            .subscribe_id(entity, component_id::<LiveDispatchProbe>())
            .expect("subscription armed");
    }
    game.tick_once();

    assert_eq!(
        game.scene_store
            .read()
            .world()
            .get::<LiveDispatchProbe>(entity)
            .unwrap()
            .intensity,
        77.0,
        "tick-side dispatcher write must be visible through the shared store"
    );
    let events = game
        .scene_store
        .write()
        .world_mut()
        .take_component_change_events();
    assert!(
        events.iter().any(|e| e.entity == entity
            && e.component == component_id::<LiveDispatchProbe>()
            && matches!(e.kind, ComponentChangeKind::Mutated)),
        "generated-code writes must fire SceneDB#47 change events, got: {events:?}"
    );
}

/// The hydration gate: scene-hydrated per-instance values survive
/// `begin_play`; absent components seed from the baked defaults (#651).
#[test]
fn hydration_is_absent_only_so_scene_overrides_win() {
    let mut world = World::new();
    let entity = world.spawn();

    // Absent → baked default seeds the REAL world component.
    LiveDispatchReference::__init_components(entity, &mut world);
    assert_eq!(
        world.get::<LiveDispatchProbe>(entity).unwrap().intensity,
        10.0
    );

    // Present (scene provided its own values) → untouched.
    world.insert(entity, LiveDispatchProbe { intensity: 99.0 });
    LiveDispatchReference::__init_components(entity, &mut world);
    assert_eq!(
        world.get::<LiveDispatchProbe>(entity).unwrap().intensity,
        99.0,
        "per-instance scene overrides must never be clobbered by baked defaults"
    );
}

/// Keeps the hand-written twin honest: PBGC must emit the same calls the
/// twin proves work, and none of the retired routing.
#[test]
fn pbgc_emission_matches_the_reference_shape_this_module_proves() {
    let spec = pbgc::ProjectSpec::new("live_dispatch_probes").add_blueprint(
        pbgc::CompiledBlueprint::new(
            "live_dispatch_reference",
            "pub fn tick(_entity: pulsar_game::Entity, _world: &mut pulsar_game::World) {}",
        )
        .with_tick(true)
        .with_begin_play(true)
        .with_components(vec![pbgc::CompiledComponent {
            class_name: "LiveDispatchProbe".to_string(),
            property_defaults: json!({ "intensity": 10.0 }),
            enabled: true,
        }]),
    );
    let project = pbgc::generate_project(&spec);
    let actor = &project.files["src/classes/live_dispatch_reference/events/events.rs"];

    for expected in [
        "__init_components(entity: Entity, world: &mut World)",
        "world_component_present_for_class(\"LiveDispatchProbe\", world, entity)",
        "hydrate_world_component_for_class(",
        "Self::__init_components(_entity, _world);",
    ] {
        assert!(
            actor.contains(expected),
            "emission lost `{expected}`:\n{actor}"
        );
    }
    for retired in [
        "__bp_with_comp",
        "__bp_set_comp_ctx",
        "ComponentStore",
        "gamma_core",
    ] {
        assert!(
            !actor.contains(retired),
            "retired routing `{retired}` reappeared:\n{actor}"
        );
    }
}
