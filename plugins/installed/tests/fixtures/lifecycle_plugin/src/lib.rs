//! The component lifecycle proof's fixture plugin (`pulsar.test_lifecycle`,
//! `plugins/installed/tests/lifecycle.rs`): one class whose lifecycle is an
//! `ActorTable`, registered from the root's `build`, so a project that
//! disables the plugin has none of it. Each event appends its name to a log
//! the test reads.

pulsar_plugins::installed!("pulsar-plugin.toml", build);

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use engine_backend::scene::class_spec::{Actor, ActorCx, ActorTable, ConstructInfo, SessionKind};
use engine_backend::scene::{ClassSpec, ComponentsBuilder};
use engine_class_derive::{engine_class, world_component};

/// The class name scripts and level files use.
pub const CLASS_NAME: &str = "LifecycleProbe";

static LOG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static CONSTRUCTS: Mutex<Vec<ConstructInfo>> = Mutex::new(Vec::new());
static KINDS: Mutex<Vec<(&'static str, SessionKind)>> = Mutex::new(Vec::new());
static CONSTRUCT_WRITES: AtomicBool = AtomicBool::new(false);

/// The events since the last call, in order.
pub fn take_log() -> Vec<&'static str> {
    std::mem::take(&mut *LOG.lock().unwrap_or_else(|poison| poison.into_inner()))
}

fn log(event: &'static str) {
    LOG.lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(event);
}

/// A component that logs its lifecycle.
#[engine_class(category = "Test", default, clone, debug, serialize, deserialize)]
pub struct LifecycleProbe {
    #[property]
    pub value: i64,
}

#[world_component]
impl LifecycleProbe {}

/// The `construct`s since the last call, as the callback was told.
pub fn take_constructs() -> Vec<ConstructInfo> {
    std::mem::take(&mut *CONSTRUCTS.lock().unwrap_or_else(|p| p.into_inner()))
}

/// The session kind each `begin_play`/`construct`/`end_play` saw, since the
/// last call.
pub fn take_kinds() -> Vec<(&'static str, SessionKind)> {
    std::mem::take(&mut *KINDS.lock().unwrap_or_else(|p| p.into_inner()))
}

/// Make `construct` write to its own component (the re-entrancy proof).
pub fn set_construct_writes(on: bool) {
    CONSTRUCT_WRITES.store(on, Ordering::SeqCst);
}

fn kind(event: &'static str, kind: SessionKind) {
    KINDS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push((event, kind));
}

impl Actor for LifecycleProbe {
    // `construct` has its own log: the begin/tick/end log above is the P1
    // proof's and stays exactly as it was.
    fn construct(&mut self, cx: &mut ActorCx<'_>) {
        kind("construct", cx.kind());
        CONSTRUCTS
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(cx.construct.clone().expect("construct says why"));
        if CONSTRUCT_WRITES.load(Ordering::SeqCst) {
            self.value += 1;
        }
    }

    fn begin_play(&mut self, cx: &mut ActorCx<'_>) {
        kind("begin_play", cx.kind());
        log("begin_play");
    }

    fn tick(&mut self, _: &mut ActorCx<'_>, _: f32) {
        log("tick");
    }

    fn end_play(cx: &mut ActorCx<'_>) {
        kind("end_play", cx.kind());
        log("end_play");
    }
}

/// Everything the plugin registers.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.components().class::<LifecycleProbe>(ClassSpec {
        actor: Some(ActorTable::of::<LifecycleProbe>()),
        ..ClassSpec::default()
    });
}
