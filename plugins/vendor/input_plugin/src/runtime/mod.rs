//! The input runtime: devices in, mappings evaluated, actions out on the
//! Gamma event bus.
//!
//! | Module | What |
//! |---|---|
//! | [`raw`] | what every key reads right now |
//! | [`modify`], [`trigger`] | modifiers and trigger state machines |
//! | [`system`] | the active mappings and the per-frame evaluation |
//! | [`events`] | the `input` event namespace, and listening to the devices' events |
//! | [`gamepad`] | controllers, read on their own thread |
//! | [`assets`] | finding the project's mappings and noticing edits |
//! | [`driver`] | one session: the above, wired to an event hub |
//! | [`component`] | `PlayerInputComponent` |
//!
//! Only `tick` here knows about the world; everything else is testable
//! without one.

pub mod assets;
pub mod component;
pub mod driver;
pub mod events;
pub mod gamepad;
pub mod modify;
pub mod raw;
pub mod system;
pub mod trigger;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pulsar_events::EventHub;
use pulsar_scenedb::{component_id, Entity, World};
use pulsar_world_registry::pulsar_scene_model::ComponentOwner;
use pulsar_world_registry::{inventory, ComponentRuntimeState, ComponentTickRegistration};

use component::{PlayerInputComponent, PLAYER_INPUT_CLASS_NAME};
use driver::{Driver, Wanted};

/// A session nobody ticked for this long is shut down.
const IDLE_SESSION: Duration = Duration::from_secs(10);

/// One driver per `World`, keyed by the world's address: tick registrations
/// are plain function pointers and carry no session state.
static SESSIONS: LazyLock<Mutex<HashMap<usize, Session>>> = LazyLock::new(Mutex::default);

struct Session {
    driver: Driver,
    last_tick: Instant,
}

fn tick(
    world: &mut World,
    events: &EventHub,
    delta_seconds: f32,
    _previous: &HashMap<Entity, Entity>,
    current: &mut HashMap<Entity, Entity>,
    _runtime: &mut ComponentRuntimeState,
) {
    profiling::profile_scope!("Input::Tick");
    let wanted: Vec<Wanted> = world
        .query::<(&PlayerInputComponent, &ComponentOwner)>()
        .filter(|(_, (component, link))| component.enabled && link.is_enabled() && !component.mapping.trim().is_empty())
        .map(|(instance, (component, link))| Wanted {
            holder: instance.bits(),
            owner: link.entity().bits(),
            mapping: component.mapping.trim().to_string(),
            priority: component.priority,
        })
        .collect();
    // Every instance is live as far as the lifecycle is concerned; the
    // driver owns what that means for input.
    for (instance, (_, link)) in world.query::<(&PlayerInputComponent, &ComponentOwner)>() {
        current.insert(instance, link.entity());
    }

    let key = world as *const World as usize;
    let mut sessions = SESSIONS.lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    sessions.retain(|&other, session| other == key || now - session.last_tick < IDLE_SESSION);
    let session = sessions.entry(key).or_insert_with(|| Session { driver: Driver::new(None, cfg!(feature = "gamepad")), last_tick: now });
    session.last_tick = now;
    session.driver.frame(events, delta_seconds, &wanted);
}

inventory::submit! {
    ComponentTickRegistration {
        type_name: "input_plugin::driver",
        class_name: PLAYER_INPUT_CLASS_NAME,
        handler_events: &[],
        component_type: component_id::<PlayerInputComponent>,
        tick,
        end_play: None,
    }
}
