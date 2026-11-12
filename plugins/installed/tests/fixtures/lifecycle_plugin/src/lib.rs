//! The component lifecycle proof's fixture plugin (`pulsar.test_lifecycle`,
//! `plugins/installed/tests/lifecycle.rs`): one class whose lifecycle is an
//! `ActorTable`, registered from the root's `build`, so a project that
//! disables the plugin has none of it. Each event appends its name to a log
//! the test reads.

pulsar_plugins::installed!("pulsar-plugin.toml", build);

use std::sync::Mutex;

use engine_backend::scene::class_spec::{Actor, ActorCx, ActorTable};
use engine_backend::scene::{ClassSpec, ComponentsBuilder};
use engine_class_derive::{engine_class, world_component};

/// The class name scripts and level files use.
pub const CLASS_NAME: &str = "LifecycleProbe";

static LOG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

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

impl Actor for LifecycleProbe {
    fn begin_play(&mut self, _: &mut ActorCx<'_>) {
        log("begin_play");
    }

    fn tick(&mut self, _: &mut ActorCx<'_>, _: f32) {
        log("tick");
    }

    fn end_play(_: &mut ActorCx<'_>) {
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
