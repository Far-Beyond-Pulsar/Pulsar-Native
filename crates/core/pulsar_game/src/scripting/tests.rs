//! The script runtime follows the world (#922): instances from
//! `ClassInstance`, runtime spawn/destroy, lifecycle order, identity, global
//! scripts and class reload, through the standalone and the PIE-style
//! shared-world paths.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use engine_backend::scene::{RuntimeLevel, SceneWorldExt, SpawnObject};
use pulsar_class::{ClassInstance, ClassRegistry};
use pulsar_core::TickMode;
use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::{
    BinOp, Constant, Function, Host, Import, Instr, Module, NativeFn, Param, Signature, Type,
    Value, Variable,
};
use serde_json::json;

use super::{global_instance_id, instance_id_for, ScriptDriver, ScriptingConfig};
use crate::tick::TickLoop;

const SPAWNER: &str = "spawner-guid";
const MINION: &str = "minion-guid";

type Log = Arc<Mutex<Vec<(String, String)>>>;

// ---- class modules ----------------------------------------------------------

fn function(name: &str, params: Vec<Type>, extra: Vec<Type>, code: Vec<Instr>) -> Function {
    let mut registers = params.clone();
    registers.extend(extra);
    Function { name: name.into(), exported: true, params, ret: Type::Unit, registers, code }
}

fn import(name: &str, params: Vec<Type>, ret: Type) -> Import {
    Import { name: name.into(), sig: Signature::new(params.into_iter().map(Param::new), ret) }
}

/// `begin_play`: `me = self`, log "begin". `end_play`: log "end".
fn minion_module() -> Module {
    let mut m = Module::new("Minion");
    m.variables = vec![Variable { name: "me".into(), ty: Type::Entity, default: None }];
    m.constants = vec![Constant::Str("begin".into()), Constant::Str("end".into())];
    m.imports = vec![import("test::event", vec![Type::Str], Type::Unit)];
    m.functions = vec![
        function("begin_play", vec![], vec![Type::Entity, Type::Str], vec![
            Instr::SelfEntity { dst: 0 },
            Instr::StoreVar { var: 0, src: 0 },
            Instr::Const { dst: 1, index: 0 },
            Instr::CallNative { import: 0, args: vec![1], dst: None },
            Instr::Return { value: None },
        ]),
        function("end_play", vec![], vec![Type::Str], vec![
            Instr::Const { dst: 0, index: 1 },
            Instr::CallNative { import: 0, args: vec![0], dst: None },
            Instr::Return { value: None },
        ]),
    ];
    m
}

/// `begin_play`: spawn five `Minion`s (by class name) at x = 0..4 into
/// `e0..e4`. `tick`: on the second tick, destroy `e1` and `e3`.
fn spawner_module() -> Module {
    let mut m = Module::new("Spawner");
    m.variables = (0..5)
        .map(|i| Variable { name: format!("e{i}"), ty: Type::Entity, default: None })
        .chain([Variable { name: "ticks".into(), ty: Type::Int, default: None }])
        .collect();
    m.constants = vec![
        Constant::Str("Minion".into()),
        Constant::Float(0.0),
        Constant::Float(1.0),
        Constant::Float(2.0),
        Constant::Float(3.0),
        Constant::Float(4.0),
        Constant::Int(1),
        Constant::Int(2),
    ];
    m.imports = vec![
        import("world::spawn", vec![Type::Str, Type::Float, Type::Float, Type::Float], Type::Entity),
        import("world::destroy", vec![Type::Entity], Type::Unit),
    ];
    let mut begin = vec![Instr::Const { dst: 0, index: 0 }, Instr::Const { dst: 2, index: 1 }];
    for i in 0..5u32 {
        begin.push(Instr::Const { dst: 1, index: 1 + i });
        begin.push(Instr::CallNative { import: 0, args: vec![0, 1, 2, 2], dst: Some(3) });
        begin.push(Instr::StoreVar { var: i, src: 3 });
    }
    begin.push(Instr::Return { value: None });
    m.functions = vec![
        function("begin_play", vec![], vec![Type::Str, Type::Float, Type::Float, Type::Entity], begin),
        function(
            "tick",
            vec![Type::Float],
            vec![Type::Int, Type::Int, Type::Bool, Type::Entity],
            vec![
                Instr::LoadVar { dst: 1, var: 5 },
                Instr::Const { dst: 2, index: 6 },
                Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
                Instr::StoreVar { var: 5, src: 1 },
                Instr::Const { dst: 2, index: 7 },
                Instr::Binary { op: BinOp::Eq, dst: 3, a: 1, b: 2 },
                Instr::Branch { cond: 3, then: 7, otherwise: 12 },
                Instr::LoadVar { dst: 4, var: 1 },
                Instr::CallNative { import: 1, args: vec![4], dst: None },
                Instr::LoadVar { dst: 4, var: 3 },
                Instr::CallNative { import: 1, args: vec![4], dst: None },
                Instr::Return { value: None },
                Instr::Return { value: None },
            ],
        ),
    ];
    m
}

// ---- project and level fixtures ---------------------------------------------

fn write_class(root: &Path, name: &str, guid: &str, module: Option<Module>, prefab: Option<serde_json::Value>) {
    let dir = root.join("src").join("classes").join(name);
    std::fs::create_dir_all(dir.join("events").join(".build")).unwrap();
    std::fs::write(dir.join("class.json"), json!({ "class_id": guid }).to_string()).unwrap();
    if let Some(module) = module {
        std::fs::write(dir.join("events/.build/module.json"), module.to_json().unwrap()).unwrap();
    }
    if let Some(prefab) = prefab {
        std::fs::write(dir.join("prefab.json"), prefab.to_string()).unwrap();
    }
}

/// A project with the `Spawner` and `Minion` classes.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_class(dir.path(), "Spawner", SPAWNER, Some(spawner_module()), None);
    write_class(dir.path(), "Minion", MINION, Some(minion_module()), None);
    dir
}

/// One level object: `(id, parent id, class (guid, name))`.
type LevelObject<'a> = (&'a str, Option<&'a str>, Option<(&'a str, &'a str)>);

/// Write a level file; parents must come before their children.
fn level(root: &Path, objects: &[LevelObject<'_>]) -> PathBuf {
    let mut components = serde_json::Map::new();
    let objects: Vec<serde_json::Value> = objects
        .iter()
        .map(|(id, parent, class)| {
            if let Some((guid, name)) = class {
                components.insert(
                    id.to_string(),
                    json!([{ "class_name": "ClassInstance", "enabled": true,
                             "data": { "class": guid, "class_name": name } }]),
                );
            }
            json!({
                "id": id, "name": id, "object_type": "Empty", "parent": parent,
                "visible": true, "locked": false, "props": {},
                "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] }
            })
        })
        .collect();
    let path = root.join("test.level");
    std::fs::write(
        &path,
        json!({ "version": "2.1", "objects": objects, "components": components }).to_string(),
    )
    .unwrap();
    path
}

/// Register `test::event(name)`, which logs `(name, StableId of self)`.
fn install_log(driver: &mut ScriptDriver) -> Log {
    let log = Log::default();
    let sink = Arc::clone(&log);
    driver
        .runtime_mut()
        .register_native(NativeFn::builder("test::event").params(["name"]).build(
            move |host: &mut Host<'_>, name: String| {
                let who = host.world.stable_id_of(host.entity).unwrap_or_default().to_owned();
                sink.lock().unwrap().push((name, who));
            },
        ))
        .unwrap();
    log
}

fn events(log: &Log) -> Vec<(String, String)> {
    log.lock().unwrap().clone()
}

fn ev(name: &str, who: &str) -> (String, String) {
    (name.to_owned(), who.to_owned())
}

// ---- the two startup paths --------------------------------------------------

/// The standalone game: `setup()` enables scripting on the loop's own
/// world, then the level is loaded into it (`windowed_app`).
fn standalone(root: &Path, level: &Path) -> (TickLoop, Log) {
    let mut game = TickLoop::new(TickMode::default(), 0);
    let log = install_log(&mut game.enable_scripting(root).lock().unwrap());
    let registry = ClassRegistry::scan(root);
    let mut store = game.scene_store.write();
    RuntimeLevel::load_into_with_classes(level, &mut store.world, &registry).unwrap();
    drop(store);
    (game, log)
}

/// Play-in-Editor: the editor's world already holds the level; the game
/// adopts it and `setup()` enables scripting (`embed.rs`).
fn pie(root: &Path, level: &Path) -> (TickLoop, Log) {
    let editor = RuntimeLevel::load_with_classes(level, &ClassRegistry::scan(root)).unwrap();
    let mut game = TickLoop::with_scene_store(editor.scene(), TickMode::default(), 0);
    let log = install_log(&mut game.enable_scripting(root).lock().unwrap());
    (game, log)
}

/// Every live instance as `(instance id, StableId of its entity)`, checking
/// each `Minion`'s `self` (its `me` variable) is its own entity.
fn instances(game: &TickLoop) -> Vec<(String, String)> {
    let driver = game.scripts.as_ref().unwrap().lock().unwrap();
    let store = game.scene_store.read();
    driver
        .runtime()
        .instance_ids()
        .iter()
        .map(|id| {
            let entity = driver.runtime().entity_of(id);
            assert_eq!(entity, driver.entity_of_instance(id), "{id}: driver lookup agrees");
            if let Some(entity) = entity {
                assert_eq!(driver.instance_of(entity), Some(id.as_str()), "{id}: entity -> instance");
            }
            if let Some(Value::Entity(me)) = driver.runtime().variable(id, "me") {
                assert_eq!(*me, entity.unwrap_or(Entity::DANGLING), "{id}: self is its own entity");
            }
            let stable = entity.and_then(|e| store.world.stable_id_of(e)).unwrap_or_default();
            (id.clone(), stable.to_owned())
        })
        .collect()
}

fn minion(stable: &str) -> (String, String) {
    (instance_id_for(stable, &MINION.into()), stable.to_owned())
}

// ---- done-when tests ---------------------------------------------------------

/// A class whose `begin_play` spawns five `Minion`s and destroys two of
/// them ends with three, each running as its own entity: `begin_play` five
/// times, `end_play` twice. Identical through the standalone and the PIE
/// path.
#[test]
fn spawn_five_destroy_two_same_in_standalone_and_pie() {
    let project = project();
    let level = level(project.path(), &[("spawner", None, Some((SPAWNER, "Spawner")))]);

    let mut outcomes = Vec::new();
    for start in [standalone as fn(&Path, &Path) -> (TickLoop, Log), pie] {
        let (mut game, log) = start(project.path(), &level);
        for _ in 0..4 {
            game.tick_once();
        }
        let begins = events(&log).iter().filter(|(e, _)| e == "begin").count();
        let ends = events(&log).iter().filter(|(e, _)| e == "end").count();
        assert_eq!((begins, ends), (5, 2));
        assert_eq!(
            events(&log),
            [
                ev("begin", "Minion_rt1"),
                ev("begin", "Minion_rt2"),
                ev("begin", "Minion_rt3"),
                ev("begin", "Minion_rt4"),
                ev("begin", "Minion_rt5"),
                ev("end", "Minion_rt2"),
                ev("end", "Minion_rt4"),
            ],
            "spawned objects begin in spawn order; destroyed ones end"
        );
        let live = instances(&game);
        assert_eq!(
            live,
            [
                (instance_id_for("spawner", &SPAWNER.into()), "spawner".to_owned()),
                minion("Minion_rt1"),
                minion("Minion_rt3"),
                minion("Minion_rt5"),
            ]
        );
        {
            let store = game.scene_store.read();
            let world = &store.world;
            assert!(world.entity_for("Minion_rt2").is_none(), "destroyed objects are gone");
            assert!(world.entity_for("Minion_rt4").is_none());
            let rt3 = world.entity_for("Minion_rt3").unwrap();
            assert_eq!(world.get::<ClassInstance>(rt3).unwrap().class.as_str(), MINION, "a real class instance");
            assert_eq!(
                world.get::<engine_backend::scene::Transform>(rt3).unwrap().position,
                [2.0, 0.0, 0.0],
                "spawned at the requested position"
            );
        }
        outcomes.push((events(&log), live));
    }
    assert_eq!(outcomes[0], outcomes[1], "standalone and PIE behave identically");
}

/// An object placed while playing (the editor placing a class during PIE)
/// starts its script on the next tick; removing it ends the script.
#[test]
fn placing_a_class_instance_during_play_starts_it_next_tick() {
    let project = project();
    let level = level(project.path(), &[("floor", None, None)]);
    let (mut game, log) = pie(project.path(), &level);
    game.tick_once();
    assert!(events(&log).is_empty());

    let def = ClassRegistry::scan(project.path()).by_name("Minion").unwrap().load_definition().unwrap();
    let placed = {
        let mut store = game.scene_store.write();
        pulsar_class::world::instantiate_class(
            &mut store.world,
            &def,
            ClassInstance::default(),
            SpawnObject::new("Minion").with_id("placed"),
        )
        .unwrap()
        .root()
    };
    assert!(events(&log).is_empty(), "nothing runs until the next tick");
    game.tick_once();
    assert_eq!(events(&log), [ev("begin", "placed")]);
    assert_eq!(instances(&game), [minion("placed")]);

    // Deleted by the editor: end_play at the next reconcile, with `self`
    // already gone (#888: no panic).
    game.scene_store.write().world.despawn_tree(placed);
    game.tick_once();
    assert_eq!(events(&log), [ev("begin", "placed"), ev("end", "")]);
    assert!(instances(&game).is_empty());
}

/// A level with no class instances runs no scripts, even though the
/// project has compiled classes (no `__vm_default` instances).
#[test]
fn a_level_without_class_instances_runs_no_scripts() {
    let project = project();
    let level = level(project.path(), &[("floor", None, None), ("wall", Some("floor"), None)]);
    for start in [standalone as fn(&Path, &Path) -> (TickLoop, Log), pie] {
        let (mut game, log) = start(project.path(), &level);
        game.tick_once();
        game.tick_once();
        assert!(instances(&game).is_empty());
        assert!(events(&log).is_empty());
    }
}

// ---- ordering, identity, globals, reload -----------------------------------

/// Level objects begin by hierarchy depth, then StableId, whatever their
/// order in the file.
#[test]
fn level_objects_begin_by_depth_then_stable_id() {
    let project = project();
    let class = Some((MINION, "Minion"));
    let level = level(
        project.path(),
        &[("b", None, class), ("c", Some("b"), class), ("a", None, class), ("aa", Some("a"), class)],
    );
    let (mut game, log) = standalone(project.path(), &level);
    game.tick_once();
    assert_eq!(events(&log), [ev("begin", "a"), ev("begin", "b"), ev("begin", "aa"), ev("begin", "c")]);
    // Shutdown ends them in start order.
    game.end_scripts();
    assert_eq!(
        events(&log)[4..],
        [ev("end", "a"), ev("end", "b"), ev("end", "aa"), ev("end", "c")]
    );
}

/// Global scripts: one unbound instance per listed class (by name or
/// GUID), started before level objects. `self` is `entity::none()`.
#[test]
fn global_scripts_get_one_unbound_instance() {
    let project = project();
    std::fs::create_dir_all(project.path().join("Pulsar")).unwrap();
    std::fs::write(
        super::scripting_config_path(project.path()),
        json!({ "global_scripts": ["Minion", MINION] }).to_string(),
    )
    .unwrap();
    assert_eq!(ScriptingConfig::load(project.path()).global_scripts.len(), 2);
    let level = level(project.path(), &[("m", None, Some((MINION, "Minion")))]);
    let (mut game, log) = standalone(project.path(), &level);
    game.tick_once();
    assert_eq!(events(&log), [ev("begin", ""), ev("begin", "m")], "listed twice, started once, first");
    assert_eq!(
        instances(&game),
        [(global_instance_id(&MINION.into()), String::new()), minion("m")]
    );
}

/// Spawn and destroy never touch the world while a script runs: both are
/// applied at the end of the script phase. The spawned entity id is
/// reserved at once; destroying it in the same frame removes it before it
/// ever starts.
#[test]
fn spawn_and_destroy_are_applied_after_the_script_phase() {
    let project = project();
    let registry = ClassRegistry::scan(project.path());
    let mut driver = ScriptDriver::with_parts(super::new_runtime(), project.path(), registry, Default::default());
    let log = install_log(&mut driver);
    let mut world = World::new();
    driver.run_frame(&mut world, 0.0);

    // A native call outside a driver frame queues nothing.
    let spawn = driver.runtime().natives().get("world::spawn").unwrap().clone();
    let destroy = driver.runtime().natives().get("world::destroy").unwrap().clone();
    let outside = spawn
        .call(&mut Host::new(&mut world, Entity::DANGLING), &mut [Value::from("Minion"), Value::Float(0.0), Value::Float(0.0), Value::Float(0.0)])
        .unwrap();
    assert_eq!(outside, Value::Entity(Entity::DANGLING));

    // Inside one: queued, applied at the end.
    let scope = super::commands::CommandScope::begin();
    let mut host = Host::new(&mut world, Entity::DANGLING);
    let kept = spawn.call(&mut host, &mut [Value::from("Minion"), Value::Float(1.0), Value::Float(0.0), Value::Float(0.0)]).unwrap();
    let gone = spawn.call(&mut host, &mut [Value::from(MINION), Value::Float(2.0), Value::Float(0.0), Value::Float(0.0)]).unwrap();
    let (Value::Entity(kept), Value::Entity(gone)) = (kept, gone) else { panic!("entities") };
    destroy.call(&mut host, &mut [Value::Entity(gone)]).unwrap();
    assert!(world.stable_id_of(kept).is_none(), "reserved, not built yet");
    let commands = scope.finish();
    assert_eq!(commands.len(), 3);
    let mut report = super::DriverReport::default();
    driver.apply_commands(&mut world, commands, &mut report);
    assert_eq!(report.spawned, [kept, gone]);
    assert_eq!(report.destroyed, [gone]);
    assert!(world.get::<ClassInstance>(kept).is_some());
    assert!(!world.is_alive(gone));

    let report = driver.run_frame(&mut world, 0.0);
    assert_eq!(report.started, [instance_id_for("Minion_rt1", &MINION.into())]);
    assert_eq!(events(&log), [ev("begin", "Minion_rt1")], "the destroyed spawn never started");
}

/// Unknown classes in `world::spawn` release the reserved entity.
#[test]
fn spawning_an_unknown_class_releases_the_entity() {
    let project = project();
    let registry = ClassRegistry::scan(project.path());
    let mut driver = ScriptDriver::with_parts(super::new_runtime(), project.path(), registry, Default::default());
    let mut world = World::new();
    let entity = world.spawn();
    let mut report = super::DriverReport::default();
    driver.apply_commands(
        &mut world,
        vec![super::WorldCommand::Spawn { entity, class: "Nope".into(), parent: None, position: [0.0; 3] }],
        &mut report,
    );
    assert!(!world.is_alive(entity));
    assert_eq!(report.failures.len(), 1);
}

/// A class update reloads the module for every live instance (state kept)
/// and binds its component slots again against the rebuilt components.
#[test]
fn class_reload_rebinds_slots_of_live_instances() {
    use helio_component::components::LightComponent;

    let project = tempfile::tempdir().unwrap();
    let light = serde_json::to_value(LightComponent::default()).unwrap();
    let prefab = json!({
        "prefab_version": 1, "name": "Lamp",
        "components": [
            { "class_name": "LightComponent", "enabled": true, "data": light },
            { "class_name": "LightComponent", "enabled": true, "data": light }
        ]
    });
    write_class(project.path(), "Lamp", "lamp-guid", None, Some(prefab));
    let registry = ClassRegistry::scan(project.path());
    let def = registry.by_name("Lamp").unwrap().load_definition().unwrap();
    let slot = def.prefab.components[1].slot_id.clone();
    let slot_var = pulsar_class::slot_variable_name(&slot);
    let mut module = Module::new("Lamp");
    module.variables = vec![
        Variable { name: slot_var.clone(), ty: Type::Component("LightComponent".into()), default: None },
        Variable { name: "kept".into(), ty: Type::Int, default: None },
    ];
    std::fs::write(def.dir.join("events/.build/module.json"), module.to_json().unwrap()).unwrap();

    let mut scene = engine_backend::scene::new_scene();
    let world = &mut scene.world;
    let root = pulsar_class::world::instantiate_class(
        world,
        &def,
        ClassInstance::default(),
        SpawnObject::new("Lamp").with_id("lamp"),
    )
    .unwrap()
    .root();
    let mut driver = ScriptDriver::with_parts(super::new_runtime(), project.path(), registry, Default::default());
    driver.run_frame(world, 0.0);
    let id = driver.instance_of(root).unwrap().to_owned();
    assert_eq!(id, "lamp::lamp-guid");
    let light_id = pulsar_world_registry::component_id_for_class("LightComponent").unwrap();
    let handle = |driver: &ScriptDriver| match driver.runtime().variable(&id, &slot_var) {
        Some(Value::Component(handle)) => handle.entity,
        other => panic!("unexpected {other:?}"),
    };
    let child = handle(&driver);
    assert_ne!(child, root, "the second light lives on a generated child");
    driver.runtime_mut().set_variable(&id, "kept", Value::Int(7)).unwrap();

    // The editor rebuilds the instance from the edited class: the child is
    // a new entity now.
    pulsar_class::world::expand_class_instance(world, root, &def);
    let rebuilt = pulsar_class::world::placement(world, root).handle(&slot).unwrap().entity;
    assert_ne!(rebuilt, child);

    let mut event = pulsar_events::AssetUpdated::new(pulsar_events::AssetKind::Blueprint);
    event.id = Some("lamp-guid".into());
    assert_eq!(driver.reload_class_for_asset(world, &event).as_deref(), Some("Lamp"));
    assert_eq!(handle(&driver), rebuilt, "slot bound to the rebuilt component");
    assert_eq!(driver.runtime().variable(&id, "kept"), Some(&Value::Int(7)), "state kept");
    assert_eq!(
        driver.runtime().variable(&id, &slot_var),
        Some(&Value::Component(pulsar_scenedb::ComponentRef::new(rebuilt, light_id)))
    );
}

/// The same through the journal: a rebuild rewrites the `ClassInstance`,
/// and the next frame rebinds the slots without restarting the script.
#[test]
fn a_rebuilt_instance_keeps_its_script_and_rebinds_next_frame() {
    let project = project();
    let registry = ClassRegistry::scan(project.path());
    let def = registry.by_name("Minion").unwrap().load_definition().unwrap();
    let mut scene = engine_backend::scene::new_scene();
    let world = &mut scene.world;
    let root = pulsar_class::world::instantiate_class(
        world,
        &def,
        ClassInstance::default(),
        SpawnObject::new("Minion").with_id("m"),
    )
    .unwrap()
    .root();
    let mut driver = ScriptDriver::with_parts(super::new_runtime(), project.path(), registry, Default::default());
    let log = install_log(&mut driver);
    driver.run_frame(world, 0.0);
    let mut instance = world.get::<ClassInstance>(root).cloned().unwrap();
    instance.variable_overrides.insert("unused".into(), json!(1));
    pulsar_class::world::store_class_instance(world, root, &instance);
    let report = driver.run_frame(world, 0.0);
    assert!(report.started.is_empty() && report.stopped.is_empty());
    assert_eq!(events(&log), [ev("begin", "m")]);

    // Swapping the class restarts it as the new class.
    let spawner = ClassInstance::new(SPAWNER.into(), "Spawner");
    pulsar_class::world::store_class_instance(world, root, &spawner);
    let report = driver.run_frame(world, 0.0);
    assert_eq!(report.stopped, [instance_id_for("m", &MINION.into())]);
    assert_eq!(report.started, [instance_id_for("m", &SPAWNER.into())]);
    assert_eq!(events(&log), [ev("begin", "m"), ev("end", "m")]);
}

/// #888: an unbound script calling component natives on `entity::none()`
/// gets an ordinary script error, not a panic, in debug builds too.
#[test]
fn component_natives_on_none_are_errors_not_panics() {
    let mut runtime = super::new_runtime();
    let of = runtime.natives().get("LightComponent::of").expect("registered").clone();
    let exists = runtime.natives().get("LightComponent::exists").expect("registered").clone();
    let getter = runtime
        .natives()
        .functions()
        .find(|f| f.name.starts_with("LightComponent::get_"))
        .expect("a LightComponent property getter")
        .clone();
    let component = Type::Component("LightComponent".into());
    let mut m = Module::new("Prober");
    m.variables = vec![Variable { name: "found".into(), ty: Type::Bool, default: None }];
    m.imports = vec![
        Import { name: of.name.clone(), sig: of.sig.clone() },
        Import { name: exists.name.clone(), sig: exists.sig.clone() },
        Import { name: getter.name.clone(), sig: getter.sig.clone() },
    ];
    m.functions = vec![function(
        "begin_play",
        vec![],
        vec![Type::Entity, component, Type::Bool, getter.sig.ret.clone()],
        vec![
            Instr::SelfEntity { dst: 0 },
            Instr::CallNative { import: 0, args: vec![0], dst: Some(1) },
            Instr::CallNative { import: 1, args: vec![1], dst: Some(2) },
            Instr::StoreVar { var: 0, src: 2 },
            Instr::CallNative { import: 2, args: vec![1], dst: Some(3) },
            Instr::Return { value: None },
        ],
    )];
    runtime.load_class(m).unwrap();
    runtime.spawn("prober", "Prober", None, &[]).unwrap();
    let mut world = World::new();
    let errors = runtime.dispatch_pending_begin_play(&mut world);
    assert_eq!(runtime.variable("prober", "found"), Some(&Value::Bool(false)));
    assert_eq!(errors.len(), 1, "the getter on none is a script error: {errors:?}");
}

// ---- script events on the engine hub (#924) --------------------------------

mod script_events {
    use super::*;
    use pulsar_events::builtin::Hit;
    use pulsar_events::gamma::Channel;
    use pulsar_events::{class_channel, entity_channel};
    use pulsar_script_vm::{EventDecl, EventField, EventRef, Subscription, SubscriptionScope};

    const TARGET: &str = "target-guid";
    const SENDER: &str = "sender-guid";

    fn handler(name: &str, params: Vec<Type>, extra: Vec<Type>, code: Vec<Instr>) -> Function {
        let mut f = function(name, params, extra, code);
        f.exported = false; // handlers need not be exported
        f
    }

    /// `var += amount` (or `+= 1` with no parameter).
    fn add_to(var: u32, param: bool) -> Vec<Instr> {
        if param {
            vec![
                Instr::LoadVar { dst: 1, var },
                Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 0 },
                Instr::StoreVar { var, src: 1 },
                Instr::Return { value: None },
            ]
        } else {
            vec![
                Instr::LoadVar { dst: 0, var },
                Instr::Const { dst: 1, index: 0 },
                Instr::Binary { op: BinOp::Add, dst: 0, a: 0, b: 1 },
                Instr::StoreVar { var, src: 0 },
                Instr::Return { value: None },
            ]
        }
    }

    /// Declares `Target.Poke(amount: int)`; handles `Hit` (self), `LevelLoaded`
    /// (global) and `Target.Poke` (self and class) by counting.
    fn target_module() -> Module {
        let mut m = Module::new("Target");
        m.variables = ["hits", "last_other", "level_loads", "pokes", "class_pokes"]
            .into_iter()
            .map(|name| Variable {
                name: name.into(),
                ty: if name == "last_other" { Type::Entity } else { Type::Int },
                default: None,
            })
            .collect();
        m.constants = vec![Constant::Int(1)];
        let mut on_hit = add_to(0, false);
        on_hit.insert(4, Instr::StoreVar { var: 1, src: 3 });
        m.functions = vec![
            // on_hit(entity, other): registers entity, other, then scratch.
            handler("on_hit", vec![Type::Entity, Type::Entity], vec![Type::Int, Type::Int], vec![
                Instr::LoadVar { dst: 2, var: 0 },
                Instr::Const { dst: 3, index: 0 },
                Instr::Binary { op: BinOp::Add, dst: 2, a: 2, b: 3 },
                Instr::StoreVar { var: 0, src: 2 },
                Instr::StoreVar { var: 1, src: 1 },
                Instr::Return { value: None },
            ]),
            handler("on_level_loaded", vec![], vec![Type::Int, Type::Int], add_to(2, false)),
            handler("on_poke", vec![Type::Int], vec![Type::Int], add_to(3, true)),
            handler("on_class_poke", vec![Type::Int], vec![Type::Int], add_to(4, true)),
        ];
        drop(on_hit);
        m.events = vec![EventDecl { name: "Target.Poke".into(), fields: vec![EventField::new("amount", Type::Int)] }];
        let sub = |event: &str, handler: u32, scope: SubscriptionScope| Subscription {
            event: EventRef::Name(event.into()),
            handler,
            scope,
        };
        m.subscriptions = vec![
            sub("Hit", 0, SubscriptionScope::Self_),
            sub("LevelLoaded", 1, SubscriptionScope::Global),
            sub("Target.Poke", 2, SubscriptionScope::Self_),
            sub("Target.Poke", 3, SubscriptionScope::Class),
        ];
        m
    }

    /// A global script: `begin_play` sends `Target.Poke(5)` to object `b`
    /// and `Target.Poke(2)` to the `Target` class; `bad()` emits
    /// `Target.Poke` with a string.
    fn sender_module() -> Module {
        let mut m = Module::new("Sender");
        m.constants = vec![
            Constant::Str("b".into()),
            Constant::Str("Target.Poke".into()),
            Constant::Int(5),
            Constant::Str("Target".into()),
            Constant::Int(2),
            Constant::Str("oops".into()),
        ];
        m.imports = vec![
            import("world::find_by_stable_id", vec![Type::Str], Type::Entity),
            import("event::send", vec![Type::Entity, Type::Str, Type::Int], Type::Unit),
            import("event::emit_to_class", vec![Type::Str, Type::Str, Type::Int], Type::Unit),
            import("event::emit", vec![Type::Str, Type::Str], Type::Unit),
        ];
        m.functions = vec![
            function("begin_play", vec![], vec![Type::Str, Type::Entity, Type::Str, Type::Int, Type::Str], vec![
                Instr::Const { dst: 0, index: 0 },
                Instr::CallNative { import: 0, args: vec![0], dst: Some(1) },
                Instr::Const { dst: 2, index: 1 },
                Instr::Const { dst: 3, index: 2 },
                Instr::CallNative { import: 1, args: vec![1, 2, 3], dst: None },
                Instr::Const { dst: 4, index: 3 },
                Instr::Const { dst: 3, index: 4 },
                Instr::CallNative { import: 2, args: vec![4, 2, 3], dst: None },
                Instr::Return { value: None },
            ]),
            function("bad", vec![], vec![Type::Str, Type::Str], vec![
                Instr::Const { dst: 0, index: 1 },
                Instr::Const { dst: 1, index: 5 },
                Instr::CallNative { import: 3, args: vec![0, 1], dst: None },
                Instr::Return { value: None },
            ]),
        ];
        m
    }

    fn events_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write_class(dir.path(), "Target", TARGET, Some(target_module()), None);
        write_class(dir.path(), "Sender", SENDER, Some(sender_module()), None);
        std::fs::create_dir_all(dir.path().join("Pulsar")).unwrap();
        std::fs::write(
            super::super::scripting_config_path(dir.path()),
            json!({ "global_scripts": ["Sender"] }).to_string(),
        )
        .unwrap();
        dir
    }

    fn target_level(root: &Path) -> PathBuf {
        let class = Some((TARGET, "Target"));
        level(root, &[("a", None, class), ("b", None, class)])
    }

    fn id(stable: &str) -> String {
        instance_id_for(stable, &TARGET.into())
    }

    fn int(game: &TickLoop, instance: &str, var: &str) -> i64 {
        let driver = game.scripts.as_ref().unwrap().lock().unwrap();
        match driver.runtime().variable(instance, var) {
            Some(Value::Int(i)) => *i,
            other => panic!("{instance}.{var}: {other:?}"),
        }
    }

    fn entity(game: &TickLoop, stable: &str) -> Entity {
        game.scene_store.read().world.entity_for(stable).unwrap()
    }

    fn subscriptions_of(game: &TickLoop, instance: &str) -> usize {
        game.scripts.as_ref().unwrap().lock().unwrap().events().unwrap().subscriptions_of(instance)
    }

    /// Declared events, sends to one entity and to a class, LevelLoaded
    /// once per instance, and a hit that reaches only its own entity.
    #[test]
    fn script_events_reach_only_their_targets() {
        let project = events_project();
        let level = target_level(project.path());
        let (mut game, _log) = standalone(project.path(), &level);
        let poke = game.events.descriptor_by_name("Target.Poke");
        assert!(poke.is_some(), "the project's events are declared when scripting is enabled");

        game.tick_once(); // a and b start; Sender sends in begin_play
        let poke = game.events.descriptor_by_name("Target.Poke").expect("declared by Target");
        assert_eq!(subscriptions_of(&game, &id("a")), 4);
        assert_eq!(
            game.events.subscriber_count(poke.id, class_channel(TARGET)),
            2,
            "both instances on their class channel"
        );
        assert_eq!(int(&game, &id("b"), "pokes"), 0, "delivered, not run, until the next script phase");

        game.tick_once(); // handlers run
        assert_eq!((int(&game, &id("a"), "pokes"), int(&game, &id("b"), "pokes")), (0, 5), "only b");
        assert_eq!((int(&game, &id("a"), "class_pokes"), int(&game, &id("b"), "class_pokes")), (2, 2));
        assert_eq!((int(&game, &id("a"), "level_loads"), int(&game, &id("b"), "level_loads")), (1, 1));

        // Physics reports a hit on a (simulated: published on its entity
        // channel, as the physics step would).
        let (a, b) = (entity(&game, "a"), entity(&game, "b"));
        game.events.publish(entity_channel(a.bits()), Hit { entity: a.bits(), other: b.bits(), impulse: 3.0 });
        game.tick_once();
        assert_eq!((int(&game, &id("a"), "hits"), int(&game, &id("b"), "hits")), (1, 0), "only a's handler");
        {
            let driver = game.scripts.as_ref().unwrap().lock().unwrap();
            assert_eq!(driver.runtime().variable(&id("a"), "last_other"), Some(&Value::Entity(b)));
        }

        // An object placed later never sees LevelLoaded; the others saw it once.
        let def = ClassRegistry::scan(project.path()).by_name("Target").unwrap().load_definition().unwrap();
        pulsar_class::world::instantiate_class(
            &mut game.scene_store.write().world,
            &def,
            ClassInstance::default(),
            SpawnObject::new("Target").with_id("late"),
        )
        .unwrap();
        for _ in 0..3 {
            game.tick_once();
        }
        assert_eq!(int(&game, &id("late"), "level_loads"), 0);
        assert_eq!((int(&game, &id("a"), "level_loads"), int(&game, &id("b"), "level_loads")), (1, 1));

        // A bad event call is a script error, not a panic.
        {
            let mut driver = game.scripts.as_ref().unwrap().lock().unwrap();
            let sender = global_instance_id(&SENDER.into());
            let mut store = game.scene_store.write();
            let err = driver.runtime_mut().send_event(&sender, "bad", &[], &mut store.world).unwrap_err();
            assert!(err.to_string().contains("Target.Poke") && err.to_string().contains("field 0"), "{err}");
        }
    }

    /// Despawning an instance drops its subscriptions (and its queued calls).
    #[test]
    fn despawning_an_instance_removes_its_subscriptions() {
        let project = events_project();
        let level = target_level(project.path());
        let (mut game, _log) = pie(project.path(), &level);
        game.tick_once();
        let hit = game.events.descriptor_by_name("Hit").unwrap().id;
        let b = entity(&game, "b");
        assert_eq!(game.events.subscriber_count(hit, entity_channel(b.bits())), 1);

        // A hit queued for b, then b is deleted before it is delivered.
        game.events.publish(entity_channel(b.bits()), Hit { entity: b.bits(), other: 0, impulse: 1.0 });
        game.scene_store.write().world.despawn_tree(b);
        game.tick_once();
        assert_eq!(subscriptions_of(&game, &id("b")), 0);
        assert_eq!(game.events.subscriber_count(hit, entity_channel(b.bits())), 0);
        let poke = game.events.descriptor_by_name("Target.Poke").unwrap().id;
        assert_eq!(game.events.subscriber_count(poke, class_channel(TARGET)), 1, "only a is left");
        let driver = game.scripts.as_ref().unwrap().lock().unwrap();
        assert!(driver.runtime().class_of(&id("b")).is_none());
        assert_eq!(driver.events().unwrap().pending_calls(), 0);
    }

    fn hub_is_clean(game: &TickLoop) {
        let driver = game.scripts.as_ref().unwrap().lock().unwrap();
        let events = driver.events().unwrap();
        assert_eq!(events.subscription_count(), 0, "no script subscriptions");
        assert_eq!(events.pending_calls(), 0, "no queued handler calls");
        assert_eq!(events.bridge().timer_count(), 0);
        assert_eq!(game.events.queued_len(), 0, "no queued events");
        let store = game.scene_store.read();
        for info in game.events.events() {
            let id = info.descriptor.id;
            assert_eq!(game.events.subscriber_count(id, Channel::Global), 0, "{}", info.descriptor.name);
            assert_eq!(game.events.subscriber_count(id, class_channel(TARGET)), 0);
            for stable in ["a", "b"] {
                let e = store.world.entity_for(stable).unwrap();
                assert_eq!(game.events.subscriber_count(id, entity_channel(e.bits())), 0);
            }
        }
    }

    /// Play, stop, play again on the editor's world: stopping leaves no
    /// subscriptions or queued events, and the second session starts fresh
    /// (LevelLoaded once per instance again).
    #[test]
    fn pie_play_stop_play_leaves_nothing_behind() {
        let project = events_project();
        let level = target_level(project.path());
        let editor = RuntimeLevel::load_with_classes(&level, &ClassRegistry::scan(project.path())).unwrap();
        for session in 0..2 {
            let mut game = TickLoop::with_scene_store(editor.scene(), TickMode::default(), 0);
            game.enable_scripting(project.path());
            for _ in 0..3 {
                game.tick_once();
            }
            assert_eq!(int(&game, &id("a"), "level_loads"), 1, "session {session}");
            assert_eq!(int(&game, &id("b"), "pokes"), 5, "session {session}");
            // Stop with work in flight: a queued hit and a queued script send.
            let a = entity(&game, "a");
            game.events.publish(entity_channel(a.bits()), Hit { entity: a.bits(), other: 0, impulse: 1.0 });
            game.end_scripts();
            hub_is_clean(&game);
        }
    }

    // ---- a plugin library publishing to scripts ----------------------------

    fn plugin() -> &'static std::sync::Mutex<libloading::Library> {
        static PLUGIN: std::sync::OnceLock<std::sync::Mutex<libloading::Library>> = std::sync::OnceLock::new();
        PLUGIN.get_or_init(|| {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../pulsar_events/tests/fixtures/plugin/Cargo.toml");
            // `<target>/debug/deps/<this test>` -> `<target>/tmp`, the
            // directory pulsar_events' own plugin test builds into.
            let exe = std::env::current_exe().unwrap();
            let target_dir = exe.ancestors().nth(3).unwrap().join("tmp").join("pulsar-events-test-plugin");
            let status = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
                .args(["build", "--release", "--quiet", "--manifest-path"])
                .arg(&manifest)
                .arg("--target-dir")
                .arg(&target_dir)
                .env_remove("RUSTFLAGS")
                .env_remove("CARGO_ENCODED_RUSTFLAGS")
                .status()
                .expect("cargo");
            assert!(status.success(), "building the test plugin failed");
            let path = target_dir.join("release").join(libloading::library_filename("pulsar_events_test_plugin"));
            std::sync::Mutex::new(unsafe { libloading::Library::new(&path) }.expect("load the test plugin"))
        })
    }

    /// A separately compiled plugin, holding the session hub through
    /// Gamma's `ForeignBus`, publishes a `Hit` (its own copy of the type):
    /// the target's script handler runs; the other instance's does not.
    #[test]
    fn a_plugin_event_reaches_a_script_handler() {
        let project = events_project();
        let level = target_level(project.path());
        let (mut game, _log) = standalone(project.path(), &level);
        game.tick_once();
        let lib = plugin().lock().unwrap();
        let attach: libloading::Symbol<unsafe extern "C" fn(pulsar_events::gamma::ffi::RawBus) -> u32> =
            unsafe { lib.get(b"fixture_attach_hub") }.unwrap();
        assert_eq!(unsafe { attach(game.events.export_raw()) }, 0);
        let publish_hit: libloading::Symbol<extern "C" fn(u64, u64, f64) -> u32> =
            unsafe { lib.get(b"fixture_publish_hit") }.unwrap();
        let (a, b) = (entity(&game, "a"), entity(&game, "b"));
        assert_eq!(publish_hit(b.bits(), a.bits(), 4.0), 0);
        game.tick_once();
        assert_eq!((int(&game, &id("a"), "hits"), int(&game, &id("b"), "hits")), (0, 1));
        {
            let driver = game.scripts.as_ref().unwrap().lock().unwrap();
            assert_eq!(driver.runtime().variable(&id("b"), "last_other"), Some(&Value::Entity(a)));
        }
        unsafe { lib.get::<extern "C" fn()>(b"fixture_shutdown").unwrap()() };
    }
}
