//! A level whose gameplay roles name classes, packaged and run headless.
//!
//! `tests/fixtures/role_game` is a project of one empty level (no objects)
//! whose World Settings pick a Blueprint class for the game mode, the game
//! state and the player controller (the role probes: each counts, in class
//! variables, the trait hooks and the lifecycle events its script is told) and
//! the Gameplay plugin's mounted `PlayerCharacter` for the character.
//!
//! The test packages it through the real path (`package`: compile every
//! language's classes, cook, write the content), then runs the packaged
//! content the way the game's `main()` does (`pulsar_game::standalone`, the
//! generated `engine_main::setup`) and checks, from the report and the world:
//!
//! - every role the level names is alive in the world as an entity of its class
//!   (an unset role is not spawned);
//! - the scripts heard `on_game_start`, `on_init`, `on_possess`, `begin_play`
//!   and `tick` (the probes' variables), in role order;
//! - an input action published on the possessed pawn's channel reaches the
//!   pawn's Blueprint (the Gameplay character jumps);
//! - the session ends every role (`end_play`) and leaves none behind.
//!
//! No window, no GPU. The game executable is not built (`skip_build`).
#![cfg(feature = "scripting")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pulsar_content::ContentRoot;
use pulsar_core::TickMode;
use pulsar_events::gamma::{Channel, DynEvent, DynValue};
use pulsar_game::standalone::{self, HeadlessHooks, HeadlessReport, HEADLESS_STEP};
use pulsar_game::tick::TickLoop;
use pulsar_package::{package, PackageOptions};

/// Standalone sessions and the plugin activation are process-wide: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const GAME_MODE: &str = "pulsar.game_mode";
const GAME_STATE: &str = "pulsar.game_state";
const PLAYER_CONTROLLER: &str = "pulsar.player_controller";
const PLAYER_CHARACTER: &str = "pulsar.player_character";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/role_game")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn int(report: &HeadlessReport, class: &str, variable: &str) -> i64 {
    report
        .instance_of(class)
        .unwrap_or_else(|| panic!("exactly one {class} is alive: {:?}", report.instances))
        .variables
        .get(variable)
        .and_then(|value| value.as_i64())
        .unwrap_or_else(|| panic!("{class}.{variable} is an int variable"))
}

fn flag(report: &HeadlessReport, class: &str, variable: &str) -> bool {
    report
        .instance_of(class)
        .unwrap_or_else(|| panic!("exactly one {class} is alive: {:?}", report.instances))
        .variables
        .get(variable)
        .and_then(|value| value.as_bool())
        .unwrap_or_else(|| panic!("{class}.{variable} is a bool variable"))
}

/// What the run saw from outside the scripts.
#[derive(Default)]
struct Seen {
    /// `(role, entity bits)` after the roles started.
    roles: Vec<(String, u64)>,
    /// The role entities alive in the world when the run ended.
    marked_at_end: usize,
    jump_before: Option<serde_json::Value>,
    jump_after: Option<serde_json::Value>,
}

fn run(content: &Path, frames: u64) -> (HeadlessReport, Seen, usize) {
    let content = ContentRoot::open(content).unwrap();
    content.install();
    let mut game = TickLoop::new(TickMode::Fixed { dt: HEADLESS_STEP }, 1);
    // What the generated `engine_main::setup()` does.
    game.enable_project_scripting().expect("scripting starts");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let hooks = HeadlessHooks {
        after_load: None,
        after_tick: Some(Box::new({
            let seen = Arc::clone(&seen);
            move |game: &mut TickLoop, tick| {
                let mut seen = seen.lock().unwrap();
                let driver = game.scripts.as_ref().expect("scripting is on");
                let (pawn, roles) = {
                    let driver = driver.lock().unwrap();
                    let roles: Vec<(String, u64)> = driver
                        .role_entities()
                        .into_iter()
                        .map(|(role, entity)| (role.to_owned(), entity.bits()))
                        .collect();
                    (driver.role_entity(PLAYER_CHARACTER), roles)
                };
                if seen.roles.len() < roles.len() {
                    seen.roles = roles;
                }
                let Some(pawn) = pawn else { return };
                let read = |game: &TickLoop| {
                    let store = game.scene_store.read();
                    pulsar_world_registry::get_component_property(
                        &store.world,
                        pawn,
                        "CharacterComponent",
                        0,
                        "jump",
                    )
                    .ok()
                };
                // Possession finishes a frame after the roles start; the
                // pawn hears input from then on.
                if tick == 6 {
                    seen.jump_before = read(game);
                    let id = game
                        .events
                        .descriptor_by_name("input::IA_Jump.triggered")
                        .expect("the project's action events are declared")
                        .id;
                    game.events
                        .publish_dyn(
                            Channel::Entity(pawn.bits()),
                            DynEvent::new(
                                id,
                                vec![
                                    DynValue::U64(pawn.bits()),
                                    DynValue::F64(1.0),
                                    DynValue::F64(1.0),
                                    DynValue::F64(0.0),
                                    DynValue::F64(0.0),
                                    DynValue::F64(0.0),
                                    DynValue::F64(0.0),
                                ],
                            ),
                        )
                        .expect("the input event publishes");
                }
                if tick == 8 {
                    seen.jump_after = read(game);
                }
            }
        })),
    };
    let report = standalone::run_headless_with(&mut game, &content, frames, hooks)
        .expect("the headless run completes");
    // After the session ended: no role entity may be left in the world.
    let left = game
        .scene_store
        .read()
        .world
        .query::<&pulsar_game::roles::RoleEntity>()
        .count();
    let mut seen = std::mem::take(&mut *seen.lock().unwrap());
    seen.marked_at_end = left;
    let roles = seen.roles.len();
    (report, seen, roles)
}

#[test]
fn a_levels_roles_are_spawned_into_the_game_and_their_scripts_hear_it() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let project = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), project.path());
    let out = tempfile::tempdir().unwrap();
    let mut options = PackageOptions::new(project.path(), out.path());
    options.loose = true;
    options.skip_build = true;
    let packaged = package(&options).unwrap_or_else(|e| panic!("packaging failed: {e}"));
    assert_eq!(packaged.startup_level.as_deref(), Some("scene/default.level"));
    // The Gameplay plugin's class is the project's too: it is in the content.
    assert!(
        out.path()
            .join("Content/plugins/Gameplay/classes/PlayerCharacter.class")
            .exists(),
        "the mounted class ships with the game"
    );

    let (report, seen, roles) = run(&out.path().join("Content"), 24);
    assert!(
        report.problems.is_empty() && report.scripts.script_errors == 0,
        "the scripts ran clean: {:?}",
        report.problems
    );

    // Every role the level names is an entity of its class, in role order.
    let names: Vec<&str> = seen.roles.iter().map(|(role, _)| role.as_str()).collect();
    assert_eq!(
        names,
        [GAME_MODE, GAME_STATE, PLAYER_CONTROLLER, PLAYER_CHARACTER],
        "{roles} roles spawned"
    );
    for class in ["RoleProbeMode", "RoleProbeState", "RoleProbePad", "PlayerCharacter"] {
        assert!(
            report.instance_of(class).is_some(),
            "{class} is alive: {:?}",
            report.instances
        );
    }

    // The trait hooks, begin_play and tick reached the scripts.
    assert_eq!(int(&report, "RoleProbeMode", "hooks"), 2, "on_game_start, on_player_joined");
    assert_eq!(int(&report, "RoleProbeState", "hooks"), 2, "on_init, on_player_added");
    assert!(
        int(&report, "RoleProbePad", "hooks") >= 3,
        "on_possess, then on_input every frame: {}",
        int(&report, "RoleProbePad", "hooks")
    );
    for class in ["RoleProbeMode", "RoleProbeState", "RoleProbePad"] {
        assert!(flag(&report, class, "began"), "{class} began play");
    }
    assert!(
        int(&report, "RoleProbeMode", "ticks") >= 20,
        "the game mode ticks every frame: {}",
        int(&report, "RoleProbeMode", "ticks")
    );

    // Input published on the pawn's channel reached its Blueprint.
    assert_eq!(seen.jump_before, Some(serde_json::json!(false)));
    assert_eq!(seen.jump_after, Some(serde_json::json!(true)));

    // The session's end took the roles with it.
    assert_eq!(seen.marked_at_end, 0, "no role entity outlives the session");
}

#[test]
fn a_level_that_names_no_roles_spawns_none() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let project = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), project.path());
    let level = project.path().join("scene/default.level");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&level).unwrap()).unwrap();
    for key in [
        "game_mode_blueprint",
        "game_state_blueprint",
        "player_controller_blueprint",
        "player_character_blueprint",
    ] {
        json["world_settings"][key] = serde_json::Value::Null;
    }
    std::fs::write(&level, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
    let out = tempfile::tempdir().unwrap();
    let mut options = PackageOptions::new(project.path(), out.path());
    options.loose = true;
    options.skip_build = true;
    package(&options).unwrap_or_else(|e| panic!("packaging failed: {e}"));
    let (report, seen, _) = run(&out.path().join("Content"), 6);
    assert!(seen.roles.is_empty(), "{:?}", seen.roles);
    assert!(
        report.instances.is_empty(),
        "no class instance: {:?}",
        report.instances
    );
}
