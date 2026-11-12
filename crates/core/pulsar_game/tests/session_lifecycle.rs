//! The session scope of the component lifecycle (P2): the kind each host path
//! reports, and the end guarantee (`end_play` for every live instance before
//! the host gets control back).

use engine_class_derive::{engine_class, register_component_runtime, register_world_component};
use pulsar_core::TickMode;
use pulsar_game::embed::PieSession;
use pulsar_game::tick::TickLoop;
use pulsar_scenedb::World;
use pulsar_world_registry::{
    attach_component, pulsar_scene_model::attachments::NewInstance, ComponentContext,
    ComponentPayload, SessionKind,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[engine_class(
    category = "Test",
    default,
    clone,
    debug,
    serialize,
    deserialize,
    no_register
)]
pub struct SessionProbe {
    #[property]
    pub value: i64,
}

#[register_world_component]
impl SessionProbe {}

static BEGUN: AtomicUsize = AtomicUsize::new(0);
static ENDED: AtomicUsize = AtomicUsize::new(0);
/// Ends are only comparable within one test: they run one after another.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

#[register_component_runtime]
impl SessionProbe {
    fn begin_play(&mut self, _: &mut ComponentContext) {
        BEGUN.fetch_add(1, Ordering::SeqCst);
    }

    fn tick(&mut self, _: &mut ComponentContext, _delta_seconds: f32) {}

    fn end_play(_: pulsar_scenedb::Entity, _: &mut ComponentContext) {
        ENDED.fetch_add(1, Ordering::SeqCst);
    }
}

fn fixed() -> TickMode {
    TickMode::Fixed {
        dt: std::time::Duration::from_millis(20),
    }
}

fn place(world: &mut World, count: usize) {
    for value in 0..count {
        let owner = world.spawn();
        attach_component(
            world,
            owner,
            NewInstance::new("SessionProbe"),
            ComponentPayload::Value(Box::new(SessionProbe {
                value: value as i64,
            })),
        )
        .unwrap();
    }
}

/// Exit criterion 5: the kind is what the host path says.
#[test]
fn each_host_path_reports_its_session_kind() {
    let standalone = TickLoop::new(fixed(), 0);
    assert_eq!(standalone.session_kind(), SessionKind::Standalone);

    let store = Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    let mut embedded = TickLoop::with_scene_store(store, fixed(), 0);
    assert_eq!(embedded.session_kind(), SessionKind::PlayInEditor);
    // The host's word (ABI v5) is what the guest sets.
    embedded.set_session_kind(SessionKind::Standalone);
    assert_eq!(embedded.session().kind(), SessionKind::Standalone);
}

/// Exit criterion 5, headless: `run_headless` (the standalone launcher's
/// `--headless` path) is a headless session.
#[test]
fn a_headless_run_is_a_headless_session() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let content = pulsar_content::ContentRoot::project(dir.path().to_path_buf());
    let mut game = TickLoop::new(fixed(), 0);
    assert_eq!(game.session_kind(), SessionKind::Standalone);
    pulsar_game::standalone::run_headless(&mut game, &content, 2).unwrap();
    assert_eq!(game.session_kind(), SessionKind::Headless);
}

/// Exit criterion 1 (the game's half): `PieSession::shutdown` runs `end_play`
/// for every live instance before it returns, which is before the editor
/// restores its snapshot.
#[test]
fn pie_shutdown_ends_every_live_instance_before_it_returns() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let store = Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    place(&mut store.write().world, 3);
    let game = TickLoop::with_scene_store(Arc::clone(&store), fixed(), 0);
    let mut session = PieSession::new(game, None);

    let (begun, ended) = (BEGUN.load(Ordering::SeqCst), ENDED.load(Ordering::SeqCst));
    session.tick();
    session.tick();
    assert_eq!(BEGUN.load(Ordering::SeqCst) - begun, 3);
    assert_eq!(ENDED.load(Ordering::SeqCst) - ended, 0, "nothing ended yet");

    session.shutdown();
    // The host restores its snapshot only now: every begin has its end.
    assert_eq!(ENDED.load(Ordering::SeqCst) - ended, 3);
    assert!(!session.tick_loop.session().has_active_components());

    // And a second shutdown, then the drop, end nothing twice.
    session.shutdown();
    drop(session);
    assert_eq!(ENDED.load(Ordering::SeqCst) - ended, 3);
}

/// A play session that is dropped without `shutdown` (a panic path) still
/// ends its instances: `TickLoop`'s drop is the fallback.
#[test]
fn dropping_the_loop_still_ends_live_instances() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let store = Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    place(&mut store.write().world, 2);
    let mut game = TickLoop::with_scene_store(Arc::clone(&store), fixed(), 0);
    let ended = ENDED.load(Ordering::SeqCst);
    game.tick_once();
    drop(game);
    assert_eq!(ENDED.load(Ordering::SeqCst) - ended, 2);
}

/// Exit criterion 7: a game that keeps running through a script-only reload
/// (the editor's `scripts_only` Play, which keeps the host) begins nothing
/// again: a class update reaches the script driver, not the component
/// lifecycle.
#[test]
fn a_script_only_reload_fires_no_begin_play() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let store = Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    place(&mut store.write().world, 2);
    let game = TickLoop::with_scene_store(Arc::clone(&store), fixed(), 0);
    let mut session = PieSession::new(game, None);
    session.tick();
    let (begun, ended) = (BEGUN.load(Ordering::SeqCst), ENDED.load(Ordering::SeqCst));

    // What `class_reload_events` hands the running game.
    session.asset_updated(pulsar_events::AssetUpdated {
        kind: pulsar_events::AssetKind::Blueprint,
        id: Some("no-such-class".into()),
        path: None,
    });
    session.tick();
    session.tick();
    assert_eq!(BEGUN.load(Ordering::SeqCst), begun, "no begin_play");
    assert_eq!(ENDED.load(Ordering::SeqCst), ended, "no end_play");
    session.shutdown();
}
