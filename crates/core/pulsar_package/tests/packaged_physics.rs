//! A packaged game runs the component lifecycle and the real physics.
//!
//! The level goes through the real cook path (`pulsar_package::package`, loose
//! `Content/`, no game binary) and is then run headless through the same code
//! the shipped game's `--headless` mode runs (`pulsar_game::standalone`:
//! `install_content`, the startup level from the cooked `project.json`,
//! `run_headless_with`), with every installed plugin linked **without** its
//! `editor` feature (as a generated game links them).
//!
//! What this proves, and what it cannot (no GPU or window here):
//!
//! - the cooked level keeps its `world_settings` and its physics component
//!   data, with content-relative paths;
//! - the session is a `Headless` one and holds the level's settings before
//!   the first tick;
//! - a dynamic body falls onto a static box and rests, a trigger reports who
//!   entered it and a collision reaches a native subscriber;
//! - the level's gravity decides the fall;
//! - the session ends: the physics thread is joined and unregistered.
//!
//! The windowed path shares the settings install (`standalone::adopt_level`)
//! and ends its session on the tick thread, which the app now joins; opening
//! a window itself is not covered here.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use engine_backend::scene::{SceneWorldExt, Transform};
use installed_plugins::physics_plugin::{
    simulated_counts, simulation_running, wait_idle, BodyType, ColliderComponent,
    ColliderShape, RigidBodyComponent, COLLISION_ENTER, TRIGGER_ENTER,
};
use pulsar_content::ContentRoot;
use pulsar_core::TickMode;
use pulsar_events::gamma::{Channel, DynValue, SubscribeOptions};
use pulsar_game::standalone::{self, HeadlessHooks, HEADLESS_STEP};
use pulsar_game::tick::TickLoop;
use pulsar_package::{package, PackageOptions};
use pulsar_world_registry::SessionKind;
use serde_json::{json, Value};

/// Physics threads are counted process-wide: one test at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn record(class: &str, value: &impl serde::Serialize) -> Value {
    json!({ "index": 0, "class_name": class, "data": serde_json::to_value(value).unwrap() })
}

fn object(id: &str, position: [f64; 3], components: Vec<Value>) -> Value {
    json!({
        "id": id, "name": id, "object_type": "Empty",
        "transform": { "position": position, "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
        "component_instances": components,
    })
}

/// The authored level: a ground slab, a crate above it, a trigger zone and a
/// ball dropped through the zone.
fn level(world_settings: Value) -> Value {
    let mut slab = ColliderComponent::default();
    slab.half_extents = [50.0, 0.5, 50.0];
    let mut zone = ColliderComponent::default();
    zone.is_trigger = true;
    zone.half_extents = [5.0, 5.0, 5.0];
    let mut zone_body = RigidBodyComponent::default();
    zone_body.body_type = BodyType::Static;
    let mut ball = ColliderComponent::default();
    ball.shape = ColliderShape::Sphere;
    json!({
        "version": "2.2",
        "objects": [
            object("ground", [0.0, 0.0, 0.0], vec![record("ColliderComponent", &slab)]),
            object("crate", [0.0, 5.0, 0.0], vec![
                record("ColliderComponent", &ColliderComponent::default()),
                record("RigidBodyComponent", &RigidBodyComponent::default()),
            ]),
            object("zone", [20.0, 0.0, 0.0], vec![
                record("ColliderComponent", &zone),
                record("RigidBodyComponent", &zone_body),
            ]),
            object("ball", [20.0, 20.0, 0.0], vec![
                record("ColliderComponent", &ball),
                record("RigidBodyComponent", &RigidBodyComponent::default()),
            ]),
        ],
        "world_settings": world_settings,
        "editor": { "camera": { "position": [0.0, 1.0, 2.0], "yaw": 0.0, "pitch": 0.0 } },
    })
}

/// A project with `level` as its default level, cooked loose. Returns the
/// content directory.
fn cook(dir: &Path, level: &Value) -> std::path::PathBuf {
    let project = dir.join("project");
    std::fs::create_dir_all(project.join("scenes")).unwrap();
    std::fs::write(
        project.join("scenes/default.level"),
        serde_json::to_vec_pretty(level).unwrap(),
    )
    .unwrap();
    let out = dir.join("out");
    let mut options = PackageOptions::new(&project, &out);
    options.loose = true;
    options.skip_build = true;
    let report = package(&options).unwrap_or_else(|e| panic!("cooking failed: {e}"));
    assert_eq!(report.startup_level.as_deref(), Some("scenes/default.level"));
    out.join("Content")
}

fn live_physics_threads() -> usize {
    editor_task_queue::thread_snapshots()
        .into_iter()
        .filter(|t| t.name == "Pulsar-Physics" && t.state != editor_task_queue::ThreadState::Exited)
        .count()
}

/// What a headless run saw.
#[derive(Default)]
struct Observed {
    crate_y: f64,
    ball_y: f64,
    collisions: usize,
    triggers: Vec<u64>,
    ball_bits: u64,
    kind: Option<SessionKind>,
    gravity_setting: Option<Value>,
    counts_while_running: (usize, usize),
    running_while_running: bool,
    live_threads_while_running: usize,
    live_threads_after: usize,
    running_after: bool,
}

/// Run the cooked content through the standalone headless entry for `frames`
/// ticks, waiting for the physics thread after each so results are exact.
fn run_cooked(content_dir: &Path, frames: u64) -> Observed {
    let content = ContentRoot::open(content_dir).unwrap();
    content.install();
    assert!(content.pak().is_none(), "the content is loose files");
    let mut game = TickLoop::new(TickMode::Fixed { dt: HEADLESS_STEP }, 1);
    let seen = Arc::new(Mutex::new(Observed::default()));
    let collisions = Arc::new(AtomicUsize::new(0));
    let triggers = Arc::new(Mutex::new(Vec::new()));
    let hooks = HeadlessHooks {
        after_load: Some(Box::new({
            let seen = Arc::clone(&seen);
            let collisions = Arc::clone(&collisions);
            let triggers = Arc::clone(&triggers);
            move |game: &mut TickLoop| {
                let mut seen = seen.lock().unwrap();
                seen.kind = Some(game.session_kind());
                seen.gravity_setting = game.session().scope().setting("gravity").cloned();
                // Idempotent: the simulation registers the same descriptors.
                let register = |descriptor| {
                    game.events
                        .register(descriptor, pulsar_events::EventCategory::Physics)
                        .expect("the physics event registers")
                };
                let collision_id = register(COLLISION_ENTER.descriptor());
                let trigger_id = register(TRIGGER_ENTER.descriptor());
                let (crate_, zone, ball) = {
                    let store = game.scene_store.read();
                    (
                        store.world.entity_for("crate").unwrap(),
                        store.world.entity_for("zone").unwrap(),
                        store.world.entity_for("ball").unwrap(),
                    )
                };
                seen.ball_bits = ball.bits();
                let bus = game.events.bus();
                let subscriptions = [
                    bus.subscribe_dyn(
                        collision_id,
                        SubscribeOptions::channel(Channel::Entity(crate_.bits())),
                        move |_| {
                            collisions.fetch_add(1, Ordering::SeqCst);
                        },
                    ),
                    bus.subscribe_dyn(
                        trigger_id,
                        SubscribeOptions::channel(Channel::Entity(zone.bits())),
                        move |event| {
                            if let DynValue::U64(other) = event.fields[2] {
                                triggers.lock().unwrap().push(other);
                            }
                        },
                    ),
                ];
                // Held until the process ends: the hub outlives the run.
                std::mem::forget(subscriptions);
            }
        })),
        after_tick: Some(Box::new({
            let seen = Arc::clone(&seen);
            move |game: &mut TickLoop, index: u64| {
                wait_idle(game.session());
                if index == 10 {
                    let mut seen = seen.lock().unwrap();
                    seen.running_while_running = simulation_running(game.session());
                    seen.counts_while_running = simulated_counts(game.session());
                    seen.live_threads_while_running = live_physics_threads();
                }
            }
        })),
    };
    standalone::run_headless_with(&mut game, &content, frames, hooks).expect("headless run");

    let store = game.scene_store.read();
    let y = |id: &str| {
        store
            .world
            .get::<Transform>(store.world.entity_for(id).unwrap())
            .unwrap()
            .position[1]
    };
    let mut seen = std::mem::take(&mut *seen.lock().unwrap());
    seen.crate_y = y("crate");
    seen.ball_y = y("ball");
    seen.collisions = collisions.load(Ordering::SeqCst);
    seen.triggers = triggers.lock().unwrap().clone();
    // The headless run ended the session: nothing of physics is left.
    seen.running_after = simulation_running(game.session());
    seen.live_threads_after = live_physics_threads();
    seen
}

#[test]
fn a_cooked_level_runs_its_physics_headless_and_ends_the_session() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let content = cook(dir.path(), &level(json!({ "gravity": [0.0, -9.81, 0.0] })));

    // The cook kept the level's settings and the physics components' data,
    // and shipped no machine path.
    let cooked: Value =
        serde_json::from_slice(&std::fs::read(content.join("scenes/default.level")).unwrap())
            .unwrap();
    assert_eq!(cooked["world_settings"]["gravity"], json!([0.0, -9.81, 0.0]));
    assert!(cooked.get("editor").is_none(), "editor data is dropped");
    let classes: Vec<_> = cooked["objects"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|o| o["component_instances"].as_array().unwrap())
        .map(|c| c["class_name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        classes.iter().filter(|c| *c == "ColliderComponent").count(),
        4
    );
    assert_eq!(
        classes.iter().filter(|c| *c == "RigidBodyComponent").count(),
        3
    );
    assert_eq!(
        cooked["objects"][2]["component_instances"][0]["data"]["is_trigger"],
        json!(true)
    );
    assert!(!cooked.to_string().contains(&dir.path().display().to_string()));

    let seen = run_cooked(&content, 300);

    assert_eq!(seen.kind, Some(SessionKind::Headless));
    assert_eq!(
        seen.gravity_setting,
        Some(json!([0.0, -9.81, 0.0])),
        "the level's settings are the session's before its first tick"
    );
    // Running: the simulation thread exists and holds every body and collider.
    assert!(seen.running_while_running, "physics started in the packaged run");
    assert_eq!(seen.counts_while_running, (3, 4));
    assert!(seen.live_threads_while_running >= 1);
    // The crate fell onto the slab (top 0.5, half-height 0.5) and rested.
    assert!(
        (0.9..1.1).contains(&seen.crate_y),
        "the crate rests on the slab: {}",
        seen.crate_y
    );
    // The ball fell through the trigger and landed on the slab (radius 0.5).
    assert!(seen.ball_y < 1.5, "the ball landed: {}", seen.ball_y);
    assert_eq!(seen.collisions, 1, "one collision_enter for the crate");
    assert_eq!(
        seen.triggers,
        vec![seen.ball_bits],
        "the trigger reported the ball once"
    );
    // The session ended: the resource is gone and the thread is joined.
    assert!(!seen.running_after);
    assert_eq!(seen.live_threads_after, 0, "no Pulsar-Physics thread leaked");
}

#[test]
fn the_cooked_levels_gravity_changes_the_fall() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fall = |settings: Value| {
        let dir = tempfile::tempdir().unwrap();
        let content = cook(dir.path(), &level(settings));
        // One second: the crate (5 m up) falls ~4.9 m on Earth, ~0.8 m on the Moon.
        let seen = run_cooked(&content, 60);
        5.0 - seen.crate_y
    };
    let earth = fall(json!({ "gravity": [0.0, -9.81, 0.0] }));
    let moon = fall(json!({ "gravity": [0.0, -1.62, 0.0] }));
    assert!(earth > 3.0, "Earth gravity: fell {earth}");
    assert!(
        moon > 0.1 && moon < earth * 0.4,
        "a weaker gravity falls less: {moon} vs {earth}"
    );
}

#[test]
fn a_level_that_turns_physics_off_never_starts_a_simulation() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let content = cook(dir.path(), &level(json!({ "physics_enabled": false })));
    let seen = run_cooked(&content, 30);
    assert!(!seen.running_while_running);
    assert_eq!(seen.crate_y, 5.0, "nothing moved the crate");
    assert_eq!(seen.live_threads_after, 0);
}
