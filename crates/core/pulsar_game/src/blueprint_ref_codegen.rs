//! #654 acceptance, sourcegen path: PBGC-generated actor shapes resolve
//! another entity's component by NAME lookup and mutate it through the
//! exact same helpers the VM trampolines call (`script_refs`), so both
//! compile targets behave identically.
//!
//! Structure mirrors `blueprint_live_dispatch`: the "twin" hand-writes the
//! emission pbgc produces for the acceptance graph and executes it against
//! a real world; a generator assertion pins that pbgc still emits those
//! very calls (drift guard), and a behavior assertion proves the resolved
//! write lands on the FOUND entity — never the executing instance.

use engine_backend::scene::WorldSceneStore;
use pulsar_scenedb::{Entity, World};
use serde_json::json;

// ── Probe component (registered like engine classes are) ────────────────

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct RefProbe {
    charges: i32,
}

impl pulsar_reflection::EngineClass for RefProbe {
    fn class_name() -> &'static str {
        "RefProbe"
    }

    fn get_properties(&self) -> Vec<pulsar_reflection::PropertyMetadata> {
        let type_info: &'static pulsar_reflection::RuntimeTypeInfo =
            RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 registered");
        vec![pulsar_reflection::PropertyMetadata {
            name: "charges",
            display_name: "Charges".into(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            type_info,
            getter: Box::new(|c: &dyn pulsar_reflection::EngineClass| {
                Box::new(c.as_any().downcast_ref::<RefProbe>().unwrap().charges)
            }),
            setter: Box::new(
                |c: &mut dyn pulsar_reflection::EngineClass, v: Box<dyn std::any::Any>| {
                    if let Some(v) = v.downcast_ref::<i32>() {
                        c.as_any_mut().downcast_mut::<RefProbe>().unwrap().charges = *v;
                    }
                },
            ),
        }]
    }

    fn get_methods() -> Vec<pulsar_reflection::MethodMetadata> {
        Vec::new()
    }

    fn create_default() -> Box<dyn pulsar_reflection::EngineClass> {
        Box::new(Self::default())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_boxed(&self) -> Box<dyn pulsar_reflection::EngineClass> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}

fn ref_probe_get(world: &World, entity: Entity) -> Option<&dyn pulsar_reflection::EngineClass> {
    world
        .get::<RefProbe>(entity)
        .map(|c| c as &dyn pulsar_reflection::EngineClass)
}

fn ref_probe_get_mut(
    world: &mut World,
    entity: Entity,
) -> Option<&mut dyn pulsar_reflection::EngineClass> {
    world
        .get_mut::<RefProbe>(entity)
        .map(|c| c.into_inner() as &mut dyn pulsar_reflection::EngineClass)
}

fn ref_probe_hydrate(
    world: &mut World,
    entity: Entity,
    data: &serde_json::Value,
) -> Result<(), String> {
    let parsed: RefProbe = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
    world.insert(entity, parsed);
    Ok(())
}

fn ref_probe_remove(world: &mut World, entity: Entity) {
    let _ = world.remove::<RefProbe>(entity);
}

pulsar_world_registry::inventory::submit! {
    pulsar_world_registry::WorldComponentRegistration {
        class_name: "RefProbe",
        component_type: pulsar_scenedb::component_id::<RefProbe>,
        hydrate: ref_probe_hydrate,
        remove: ref_probe_remove,
        dispatch: |world, entity, _: _, _: usize, _: _| {
            world.get::<RefProbe>(entity).is_some()
        },
        get_as_engine_class: ref_probe_get,
        get_as_engine_class_mut: ref_probe_get_mut,
        on_removed: |_, _| {},
        refresh_gpu_mirror: |_, _| {},
    }
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::EngineClassRegistration {
        name: "RefProbe",
        category: None,
        constructor: <RefProbe as pulsar_reflection::EngineClass>::create_default,
        from_json: None,
    }
}

use pulsar_reflection::RUNTIME_TYPE_REGISTRY;

/// A two-object editor-hydrated-shaped scene: a trigger (the executing
/// instance's actor) and the lamp it references by display name.
fn scene() -> (WorldSceneStore, Entity, Entity) {
    let mut store = WorldSceneStore::new();
    let trigger = store
        .spawn(Some("trigger".into()), "Trigger", None)
        .expect("spawn trigger");
    let lamp = store
        .spawn(Some("lamp".into()), "Red Lamp", None)
        .expect("spawn lamp");
    store.world_mut().insert(trigger, RefProbe { charges: 1 });
    store.world_mut().insert(lamp, RefProbe { charges: 2 });
    (store, trigger, lamp)
}

/// The generated-shape twin of the acceptance graph:
///
/// ```text
/// begin_play ─▶ find_object_by_name("Red Lamp")
///              ─▶ get_component_ref::RefProbe::0  (actor ◀── found)
///              ─▶ comp_set_prop::RefProbe::charges := 99  (component_ref ◀── ref)
/// ```
///
/// Each step calls EXACTLY what rust_codegen emits for these nodes (see
/// `pbgc_emission_matches_the_twin_calls` below): the resolver produces the
/// actor, `component_ref_json` builds the reference ON it, and
/// `resolve_pin_target` + the dispatcher perform the pin-targeted write.
/// Only the surrounding function shell is elided — compiled logic already
/// runs inside an Actor callback carrying `(_entity, _world)`.
fn generated_begin_play_twin(_entity: Entity, _world: &mut World) {
    let actor_bits =
        crate::script_refs::find_object_by_name(_world, &json!("Red Lamp"), "find_object_by_name");
    let Some(actor) = actor_bits.as_u64().map(pulsar_scenedb::Entity::from_bits) else {
        tracing::error!("find_object_by_name resolved nothing; event degrades");
        return;
    };
    let reference = crate::script_refs::component_ref_json(
        _world,
        actor,
        "RefProbe",
        0,
        "get_component_ref::RefProbe::0",
    );
    match crate::script_refs::resolve_pin_target(
        _world,
        &reference,
        "RefProbe",
        "comp_set_prop::RefProbe::charges",
    ) {
        Some((__bp_target_entity, __bp_target_index)) => {
            if let Err(__e) = pulsar_world_registry::dispatch::set_component_property(
                _world,
                __bp_target_entity,
                "RefProbe",
                __bp_target_index,
                "charges",
                serde_json::to_value(99).unwrap_or(serde_json::Value::Null),
            ) {
                tracing::error!("comp_set_prop::RefProbe::charges failed: {__e}");
            }
        }
        None => {}
    }
}

/// THE behavior assertion: same outcome as the VM-path test
/// (`component_ops::tests::cross_object_chain_resolves_and_writes_the_found_entity`)
/// — the found entity is written, the executing instance is not.
#[test]
fn sourcegen_twin_writes_the_found_entity_like_the_vm_path() {
    let (mut store, trigger, lamp) = scene();

    // Generated actors receive (_entity, _world) straight from the Actor
    // callback — no VM context involved on this path.
    let world = store.world_mut();
    generated_begin_play_twin(trigger, world);

    assert_eq!(
        store.world().get::<RefProbe>(lamp).unwrap().charges,
        99,
        "found entity written through the resolved reference"
    );
    assert_eq!(
        store.world().get::<RefProbe>(trigger).unwrap().charges,
        1,
        "executing instance untouched"
    );
}

/// Drift guard: pbgc's Rust emission must route identity nodes through
/// `pulsar_game::script_refs` and pin-targeted ops through
/// `resolve_pin_target`, matching the twin above.
#[test]
fn pbgc_emission_matches_the_twin_calls() {
    use pbgc::{
        compile_graph, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
        PinType, Position,
    };

    fn begin(id: &str) -> NodeInstance {
        let mut n = NodeInstance::new(id, "begin_play", Position { x: 0.0, y: 0.0 });
        n.outputs.push(PinInstance::new(
            format!("{id}_o"),
            Pin::new(format!("{id}_o"), "Body", DataType::Exec, PinType::Output),
        ));
        n
    }

    let mut g = GraphDescription::new("ref_acceptance");
    g.add_node(begin("be"));

    let mut find = NodeInstance::new("find", "find_object_by_name", Position { x: 10.0, y: 0.0 });
    find.inputs.push(PinInstance::new(
        "find_n",
        Pin::new("find_n", "name", DataType::typed("String"), PinType::Input),
    ));
    find.outputs.push(PinInstance::new(
        "find_r",
        Pin::new(
            "find_r",
            "actor",
            DataType::typed("ActorRef"),
            PinType::Output,
        ),
    ));
    find.properties.insert("find_n".into(), json!("Red Lamp"));
    g.add_node(find);

    let mut get_ref = NodeInstance::new(
        "gr",
        "get_component_ref::RefProbe::0",
        Position { x: 20.0, y: 0.0 },
    );
    get_ref.inputs.push(PinInstance::new(
        "actor",
        Pin::new(
            "actor",
            "actor",
            DataType::typed("ActorRef"),
            PinType::Input,
        ),
    ));
    get_ref.outputs.push(PinInstance::new(
        "gr_r",
        Pin::new(
            "gr_r",
            "component",
            DataType::typed("ComponentRef"),
            PinType::Output,
        ),
    ));
    g.add_node(get_ref);

    let mut set = NodeInstance::new(
        "set",
        "comp_set_prop::RefProbe::charges",
        Position { x: 30.0, y: 0.0 },
    );
    set.inputs.push(PinInstance::new(
        "set_e",
        Pin::new("set_e", "exec", DataType::Exec, PinType::Input),
    ));
    set.inputs.push(PinInstance::new(
        "component_ref",
        Pin::new(
            "component_ref",
            "component",
            DataType::typed("ComponentRef"),
            PinType::Input,
        ),
    ));
    set.inputs.push(PinInstance::new(
        "set_v",
        Pin::new("set_v", "value", DataType::typed("i32"), PinType::Input),
    ));
    set.outputs.push(PinInstance::new(
        "set_o",
        Pin::new("set_o", "exec", DataType::Exec, PinType::Output),
    ));
    set.properties.insert("set_v".into(), json!(99));
    g.add_node(set);

    let mut data = |from: &str, fp: &str, to: &str, tp: &str| {
        g.connections.push(pbgc::Connection {
            source_node: from.into(),
            source_pin: fp.into(),
            target_node: to.into(),
            target_pin: tp.into(),
            connection_type: ConnectionType::Data,
        });
    };
    data("find", "find_r", "gr", "actor");
    data("gr", "gr_r", "set", "component_ref");
    g.connections.push(pbgc::Connection {
        source_node: "be".into(),
        source_pin: "be_o".into(),
        target_node: "set".into(),
        target_pin: "set_e".into(),
        connection_type: ConnectionType::Execution,
    });

    let logic = compile_graph(&g).expect("acceptance graph compiles");
    for expected in [
        "pulsar_game::script_refs::find_object_by_name(",
        "pulsar_game::script_refs::component_ref_json(",
        "pulsar_game::script_refs::resolve_pin_target(",
        "pulsar_world_registry::dispatch::set_component_property(",
        "__bp_target_index,",
    ] {
        assert!(
            logic.contains(expected),
            "emission drifted from the twin; missing `{expected}`:\n{logic}"
        );
    }
}
