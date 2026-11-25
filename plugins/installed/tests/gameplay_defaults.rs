//! `pulsar.gameplay`: the gameplay roles as scene defaults and the default
//! player character, which goes through the roles system like any class.
//!
//! - Enabled, the plugin registers the four roles; it registers no native
//!   default, so a level that selects no character spawns none.
//! - Its folder, `plugins/Gameplay/`, mounts read-only in a project and holds
//!   `PlayerCharacter.class`: found as a class, listed in the trait index as an
//!   implementer of the Player Character trait, and valid as that role's
//!   selection (what World Settings stores when a level picks it).
//! - Input events on the possessed pawn's entity channel move its character.

use std::sync::Mutex;

use installed_plugins as _;

use gameplay_plugin::files::{DEFAULT_MAPPING, FILES, PLAYER_CHARACTER_GRAPH};
const CHARACTER_CLASS: &str = "CharacterComponent";
use pulsar_events::gamma::{Channel, DynEvent, DynValue};
use pulsar_events::{EventHub, FlushPoint};
use pulsar_scenedb::{Entity, World};
use pulsar_traits::builtin::{
    GAME_MODE_ROLE, GAME_STATE_ROLE, PLAYER_CHARACTER_ROLE, PLAYER_CHARACTER_TRAIT_ID,
    PLAYER_CONTROLLER_ROLE,
};
use pulsar_traits::{BindingStatus, RoleSelection, TraitIndex, WorldRoles};

/// The tests change the process's activated plugins: one at a time.
static ACTIVATION: Mutex<()> = Mutex::new(());

fn everything_on() {
    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default(),
    ));
}

#[test]
fn the_plugin_registers_the_four_roles_and_no_native_default() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let ids: Vec<_> = pulsar_traits::roles::roles().iter().map(|r| r.id).collect();
    for role in [
        GAME_MODE_ROLE,
        GAME_STATE_ROLE,
        PLAYER_CONTROLLER_ROLE,
        PLAYER_CHARACTER_ROLE,
    ] {
        assert!(ids.contains(&role), "{role} is registered: {ids:?}");
    }
}

#[test]
fn the_shipped_class_is_a_player_character_blueprint() {
    let file = |name: &str| {
        let file = FILES
            .iter()
            .find(|file| file.path == format!("classes/PlayerCharacter.class/{name}"))
            .unwrap_or_else(|| panic!("{name} is shipped"));
        std::str::from_utf8(file.bytes).unwrap().to_owned()
    };
    let class: serde_json::Value = serde_json::from_str(&file("class.json")).unwrap();
    assert!(class["class_id"].as_str().is_some_and(|id| id.len() == 36));
    let prefab: serde_json::Value = serde_json::from_str(&file("prefab.json")).unwrap();
    let classes: Vec<_> = prefab["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["class_name"].as_str().unwrap())
        .collect();
    assert_eq!(
        classes,
        [
            CHARACTER_CLASS,
            "CharacterControllerComponent",
            "PlayerInputComponent",
            "CameraComponent"
        ]
    );
    assert!(PLAYER_CHARACTER_GRAPH.ends_with("classes/PlayerCharacter.class/graph_save.json"));
}

#[test]
fn the_mounted_class_can_be_selected_for_the_player_character_role() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let project = tempfile::tempdir().unwrap();
    let root = project.path();

    // The plugin's folder is mounted read-only in the project.
    engine_fs::virtual_fs::mount_project(root);
    let class_json = root.join("plugins/Gameplay/classes/PlayerCharacter.class/class.json");
    assert!(
        engine_fs::virtual_fs::read_to_string(&class_json).is_ok(),
        "the plugin's folder is mounted: {:?}",
        engine_fs::virtual_fs::mount_problems(root)
    );
    let refused = engine_fs::virtual_fs::write_file(&class_json, b"{}").unwrap_err();
    assert!(engine_fs::virtual_fs::is_read_only(&refused), "{refused}");

    // It is a class of the project.
    let registry = pulsar_class::ClassRegistry::scan(root);
    let names: Vec<_> = registry.entries().iter().map(|e| e.name.clone()).collect();
    assert!(
        names.iter().any(|name| name == "PlayerCharacter"),
        "{names:?}"
    );

    // It is an implementer of the role's trait, and a valid selection.
    let index = TraitIndex::rebuild(root).unwrap();
    let implementers: Vec<_> = index
        .implementers_of(PLAYER_CHARACTER_TRAIT_ID)
        .into_iter()
        .map(|entry| entry.id.clone())
        .collect();
    assert!(
        implementers.iter().any(|id| id == PLAYER_CHARACTER_GRAPH),
        "{implementers:?}"
    );
    let mut selection = RoleSelection::default();
    selection.set(
        PLAYER_CHARACTER_ROLE,
        Some(PLAYER_CHARACTER_GRAPH.to_owned()),
    );
    let roles = WorldRoles::resolve(&selection, Some(&index));
    assert_eq!(
        roles
            .get(PLAYER_CHARACTER_ROLE)
            .map(|binding| binding.status),
        Some(BindingStatus::Valid),
        "{:?}",
        roles.diagnostics()
    );
    engine_fs::virtual_fs::unmount_project(root);
}

/// A process that did not open the project (a plugin library has its own copy of
/// `engine_fs`, with none of the host's mounts) still finds and compiles the plugin's
/// mounted class: listing classes and compiling a project mount it. Without that the
/// packager's script step failed with "has graph_save.json source but no compiled
/// module" for `plugins/Gameplay/classes/PlayerCharacter.class`.
#[test]
fn the_mounted_class_is_found_and_compiled_without_the_project_being_mounted_first() {
    use pulsar_class::{find_class_dirs, ClassRegistry, ModuleStore};
    use pulsar_language::LanguageRegistry;

    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::create_dir_all(root.join("Pulsar")).unwrap();
    // Not mounted: nothing in this process opened the project.
    let dirs = find_class_dirs(root);
    assert!(
        dirs.iter().any(|dir| dir.ends_with("PlayerCharacter.class")),
        "{dirs:?}"
    );
    engine_fs::virtual_fs::unmount_project(root);

    let natives = pulsar_script_vm::NativeRegistry::with_engine_natives();
    for (language, compiler) in LanguageRegistry::current().compilers() {
        let problems = compiler.compile_project(root, &natives);
        assert!(problems.is_empty(), "{}: {problems:?}", language.id);
    }
    let registry = ClassRegistry::scan(root);
    let entry = registry.by_name("PlayerCharacter").cloned().unwrap();
    assert!(ModuleStore::of_project(root).has_module(&entry.id));
    engine_fs::virtual_fs::unmount_project(root);
}

fn action_event(hub: &EventHub, name: &str, phase: &str, pawn: Entity, x: f64, y: f64) {
    let id = hub
        .descriptor_by_name(&format!("input::{name}.{phase}"))
        .unwrap()
        .id;
    hub.publish_dyn(
        Channel::Entity(pawn.bits()),
        DynEvent::new(
            id,
            vec![
                DynValue::U64(pawn.bits()),
                DynValue::F64((x * x + y * y).sqrt()),
                DynValue::F64(x),
                DynValue::F64(y),
                DynValue::F64(0.0),
                DynValue::F64(0.0),
                DynValue::F64(0.0),
            ],
        ),
    )
    .unwrap();
}

fn read(world: &World, pawn: Entity, property: &str) -> serde_json::Value {
    pulsar_world_registry::get_component_property(world, pawn, CHARACTER_CLASS, 0, property)
        .unwrap()
}

#[test]
fn a_level_that_selects_the_mounted_class_spawns_it_through_the_roles_system() {
    use pulsar_class::{ClassRegistry, ModuleStore};
    use pulsar_game::scripting::{new_runtime, ScriptDriver};
    use pulsar_language::LanguageRegistry;
    use pulsar_world_registry::pulsar_scene_model::attachments;

    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    // A project is a folder with a `Pulsar` directory: where its module store is.
    std::fs::create_dir_all(root.join("Pulsar")).unwrap();
    engine_fs::virtual_fs::mount_project(root);

    let natives = pulsar_script_vm::NativeRegistry::with_engine_natives();
    for (language, compiler) in LanguageRegistry::current().compilers() {
        let problems = compiler.compile_project(root, &natives);
        assert!(problems.is_empty(), "{}: {problems:?}", language.id);
    }
    let registry = ClassRegistry::scan(root);
    let entry = registry.by_name("PlayerCharacter").cloned().unwrap();
    assert!(
        ModuleStore::of_project(root).has_module(&entry.id),
        "the read-only class compiled into the project's module store (compilers: {:?})",
        LanguageRegistry::current()
            .compilers()
            .iter()
            .map(|(l, _)| l.id)
            .collect::<Vec<_>>()
    );

    let index = TraitIndex::rebuild(root).unwrap();
    let mut selection = RoleSelection::default();
    selection.set(
        PLAYER_CHARACTER_ROLE,
        Some(PLAYER_CHARACTER_GRAPH.to_owned()),
    );
    let roles = WorldRoles::resolve(&selection, Some(&index));
    let mut driver = ScriptDriver::with_parts(new_runtime(), root, registry, Default::default());
    driver.set_world_roles(roles);
    let mut world = World::new();
    for _ in 0..3 {
        let frame = driver.run_frame(&mut world, 0.0);
        assert!(frame.failures.is_empty(), "{:?}", frame.failures);
    }
    let pawn = driver
        .role_entity(PLAYER_CHARACTER_ROLE)
        .expect("the role's entity was spawned");
    assert!(driver.instance_of(pawn).is_some(), "its script runs");
    let classes: Vec<String> = attachments::instances(&world, pawn)
        .iter()
        .filter_map(|instance| attachments::meta(&world, *instance).map(|m| m.class_name.clone()))
        .collect();
    for expected in [CHARACTER_CLASS, "CameraComponent"] {
        assert!(
            classes.iter().any(|c| c == expected),
            "{expected}: {classes:?}"
        );
    }

    // The class's own Blueprint moves it. The project's action events are declared
    // when the session attaches to the hub; the plugin's mounted actions are the
    // project's, so IA_Move and the rest exist without any file of the project's.
    let hub = EventHub::new();
    driver.attach_events(hub.clone());
    driver.run_frame(&mut world, 0.0);
    let send = |driver: &mut ScriptDriver,
                world: &mut World,
                action: &str,
                phase: &str,
                x: f64,
                y: f64| {
        action_event(&hub, action, phase, pawn, x, y);
        hub.flush(FlushPoint::Manual);
        let frame = driver.run_frame(world, 0.0);
        assert!(frame.script_errors.is_empty(), "{:?}", frame.script_errors);
    };
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 0.0])
    );

    // Move: held forward and to the right, then released.
    send(&mut driver, &mut world, "IA_Move", "triggered", 0.5, 1.0);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.5, 1.0])
    );
    send(&mut driver, &mut world, "IA_Move", "completed", 0.0, 0.0);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 0.0])
    );

    // Jump: asks the character to jump.
    assert_eq!(read(&world, pawn, "jump"), serde_json::json!(false));
    send(&mut driver, &mut world, "IA_Jump", "triggered", 1.0, 0.0);
    assert_eq!(read(&world, pawn, "jump"), serde_json::json!(true));

    // Sprint: faster while held, the walk speed again when released.
    assert_eq!(read(&world, pawn, "walk_speed"), serde_json::json!(4.0));
    send(&mut driver, &mut world, "IA_Sprint", "started", 1.0, 0.0);
    assert_eq!(read(&world, pawn, "walk_speed"), serde_json::json!(7.0));
    send(&mut driver, &mut world, "IA_Sprint", "completed", 0.0, 0.0);
    assert_eq!(read(&world, pawn, "walk_speed"), serde_json::json!(4.0));

    // Look: the horizontal axis turns the CONTROL rotation (the view), not the
    // body: the body follows by the character's rotation options.
    let control_yaw = |world: &World| {
        pulsar_world_registry::get_component_property(world, pawn, "CharacterComponent", 0, "control_yaw")
            .unwrap()
            .as_f64()
            .unwrap()
    };
    let before = control_yaw(&world);
    send(&mut driver, &mut world, "IA_Look", "triggered", 10.0, 3.0);
    assert!(
        (control_yaw(&world) - (before - 10.0)).abs() < 1e-3,
        "turned by the axis times turn_rate: {before} -> {}",
        control_yaw(&world)
    );

    // Look up and down: the vertical axis pitches the camera (positive looks up),
    // and the camera holds its limits however far the stick is pushed.
    let pitch = |world: &World| {
        pulsar_world_registry::get_component_property(world, pawn, "CharacterComponent", 0, "control_pitch")
            .unwrap()
            .as_f64()
            .unwrap()
    };
    assert_eq!(pitch(&world), 3.0, "IA_Look y=3 looked up three degrees");
    send(&mut driver, &mut world, "IA_Look", "triggered", 0.0, -5.0);
    assert_eq!(pitch(&world), -2.0, "a negative axis looks down");
    send(&mut driver, &mut world, "IA_Look", "triggered", 0.0, 500.0);
    assert_eq!(pitch(&world), 80.0, "looking up stops at the camera's upper limit");
    send(&mut driver, &mut world, "IA_Look", "triggered", 0.0, -1000.0);
    assert_eq!(pitch(&world), -80.0, "looking down stops at the lower limit");

    // Only a wired phase is listened to: IA_Jump.started has no chain, so it
    // changes nothing (jump is already set, so this only checks it does not fail).
    send(&mut driver, &mut world, "IA_Jump", "started", 1.0, 0.0);
    assert_eq!(read(&world, pawn, "jump"), serde_json::json!(true));
    engine_fs::virtual_fs::unmount_project(root);
}

/// By default nothing but the free camera: with no character selected the role
/// is skipped, not defaulted.
#[test]
fn a_level_that_selects_nothing_spawns_no_character() {
    use pulsar_class::ClassRegistry;
    use pulsar_game::scripting::{new_runtime, ScriptDriver};

    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    engine_fs::virtual_fs::mount_project(root);
    let mut driver = ScriptDriver::with_parts(
        new_runtime(),
        root,
        ClassRegistry::scan(root),
        Default::default(),
    );
    driver.set_world_roles(WorldRoles::resolve(&RoleSelection::default(), None));
    let mut world = World::new();
    for _ in 0..3 {
        driver.run_frame(&mut world, 0.0);
    }
    assert!(driver.role_entity(PLAYER_CHARACTER_ROLE).is_none());
    assert!(driver.role_entities().is_empty());
    engine_fs::virtual_fs::unmount_project(root);
}

/// Look is immediate: the shipped IA_Look adds no smoothing (a time constant
/// delays every mouse movement by a fraction of a second) and no response
/// curve or threshold that drops slow movements.
#[test]
fn the_look_action_adds_no_delay_and_drops_no_slow_movement() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../vendor/gameplay/assets/input/actions/IA_Look.inputaction");
    let action: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(action["modifiers"], serde_json::json!([]), "no smooth, no curve");
    assert!(
        action["actuation_threshold"].as_f64().unwrap() <= 0.001,
        "one pixel of mouse movement is an input"
    );
}

/// The default character's graph calls methods of the character plugin
/// (`add_control_yaw`, `add_control_pitch`, ...). The two plugins are separate
/// libraries, so an older character library next to a newer gameplay library
/// failed the pack step with "no native function ... is registered". The
/// gameplay plugin pins the character contract's version, so the older library
/// is refused by name when plugins resolve. Bump both when the graph needs a
/// new method.
#[test]
fn gameplay_pins_the_character_contract_version_its_graph_needs() {
    let vendor = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../vendor");
    let read = |name: &str| -> toml::Value {
        toml::from_str(&std::fs::read_to_string(vendor.join(name).join("pulsar-plugin.toml")).unwrap())
            .unwrap()
    };
    let provided = read("character")["provides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str() == Some("pulsar.character.classes"))
        .and_then(|c| c["version"].as_integer())
        .unwrap();
    let required = read("gameplay")["requires"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str() == Some("pulsar.character.classes"))
        .and_then(|c| c.get("version").and_then(toml::Value::as_integer))
        .expect("gameplay pins the character contract version");
    assert_eq!(provided, required);

    // Every character method the graph calls exists in the registry the pack
    // step compiles against.
    let graph = std::fs::read_to_string(
        vendor.join("gameplay/assets/classes/PlayerCharacter.class/graph_save.json"),
    )
    .unwrap();
    let natives = pulsar_script_vm::NativeRegistry::with_engine_natives();
    let mut seen = 0;
    for part in graph.split('"') {
        if let Some(rest) = part.strip_prefix("comp_call::CharacterComponent::") {
            let name = format!("CharacterComponent::{rest}");
            assert!(natives.get(&name).is_some(), "`{name}` is not a registered native");
            seen += 1;
        }
    }
    assert!(seen >= 5, "the graph calls the character's methods ({seen})");
}
