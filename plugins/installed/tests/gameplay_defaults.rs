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

use gameplay_plugin::files::{FILES, PLAYER_CHARACTER_GRAPH};
use gameplay_plugin::player_input::{drive, CHARACTER_CLASS};
use installed_plugins::input_plugin::runtime::events::action_descriptor;
use installed_plugins::input_plugin::runtime::system::Phase;
use pulsar_events::gamma::{Channel, DynEvent, DynValue};
use pulsar_events::{EventCategory, EventHub, FlushPoint};
use pulsar_scenedb::{Entity, World};
use pulsar_traits::builtin::{
    GAME_MODE_ROLE, GAME_STATE_ROLE, PLAYER_CHARACTER_ROLE, PLAYER_CHARACTER_TRAIT_ID,
    PLAYER_CONTROLLER_ROLE,
};
use pulsar_traits::{BindingStatus, RoleSelection, TraitIndex, WorldRoles};
use pulsar_world_registry::pulsar_scene_model::attachments::NewInstance;
use pulsar_world_registry::{place_component, ComponentPayload};

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

fn action_event(hub: &EventHub, name: &str, phase: Phase, pawn: Entity, x: f64, y: f64) {
    let id = hub
        .descriptor_by_name(&format!("input::{name}.{}", phase.word()))
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

fn possess(hub: &EventHub, pawn: Entity, possessed: bool) {
    let id = hub.descriptor_by_name("PossessionChanged").unwrap().id;
    hub.publish_dyn(
        Channel::Global,
        DynEvent::new(
            id,
            vec![
                DynValue::U64(0),
                DynValue::U64(pawn.bits()),
                DynValue::Bool(possessed),
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
fn the_possessed_pawns_move_and_jump_actions_drive_its_character() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let mut world = World::new();
    let pawn = {
        use engine_backend::scene::{SceneWorldExt, SpawnObject};
        let pawn = world.spawn_object(SpawnObject::new("Pawn")).unwrap();
        place_component(
            &mut world,
            pawn,
            NewInstance::new(CHARACTER_CLASS),
            ComponentPayload::Default,
        )
        .unwrap();
        pawn
    };

    let hub = EventHub::new();
    for (name, phase) in [
        ("Move", Phase::Triggered),
        ("Move", Phase::Completed),
        ("Jump", Phase::Started),
    ] {
        hub.register(
            action_descriptor(name, phase).unwrap(),
            EventCategory::Input,
        )
        .unwrap();
    }
    let resolve = |_: Entity, _: &str| None;

    // Nothing possessed: nothing moves.
    drive(&mut world, &hub, &resolve);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 0.0])
    );

    // The driver announces the possession; the next frame subscribes the pawn.
    possess(&hub, pawn, true);
    hub.flush(FlushPoint::Manual);
    drive(&mut world, &hub, &resolve);

    action_event(&hub, "Move", Phase::Triggered, pawn, 0.0, 1.0);
    hub.flush(FlushPoint::Manual);
    drive(&mut world, &hub, &resolve);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 1.0])
    );

    action_event(&hub, "Jump", Phase::Started, pawn, 1.0, 0.0);
    hub.flush(FlushPoint::Manual);
    drive(&mut world, &hub, &resolve);
    assert_eq!(read(&world, pawn, "jump"), serde_json::json!(true));

    action_event(&hub, "Move", Phase::Completed, pawn, 0.0, 0.0);
    hub.flush(FlushPoint::Manual);
    drive(&mut world, &hub, &resolve);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 0.0])
    );

    // Another object's input is not this pawn's.
    let other = world.spawn();
    action_event(&hub, "Move", Phase::Triggered, other, 1.0, 0.0);
    hub.flush(FlushPoint::Manual);
    drive(&mut world, &hub, &resolve);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 0.0])
    );
}

/// A level that selects the plugin's character gets it from the roles system:
/// the mounted class compiles into the project's module store (the class
/// folder is read-only), the driver spawns it as the role's entity with its
/// prefab's components, and its script runs.
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
