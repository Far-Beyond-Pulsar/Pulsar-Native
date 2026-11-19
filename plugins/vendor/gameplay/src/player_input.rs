//! The player's input to the possessed character.
//!
//! The input plugin's possession hook puts the level's mapping to work on
//! the pawn, and its events (`input::Move.*`, `input::Jump.started`) arrive on
//! the pawn's entity channel. This hears them there and writes the character's
//! `move_input` and `jump`, which the character plugin's own system turns into
//! movement. `Move` is x right, y forward, as the example mapping's WASD and
//! stick bindings produce it.
//!
//! Which pawn is the kernel's to say: the driver publishes `PossessionChanged`
//! when the local player's controller takes or releases one, and this follows
//! it. Whatever class the level selected for the Player Character role is the
//! pawn (a project's own, or this plugin's `PlayerCharacter` class); anything
//! with a `CharacterComponent` on it moves.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine_backend::scene::game_hooks::{ScriptVariableResolver, TickPhaseRegistration};
use pulsar_events::gamma::{Channel, DynEvent, DynValue, SubscribeOptions, SyncSubscription};
use pulsar_events::EventHub;
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::pulsar_scene_model::attachments;

/// The character plugin's class: what moves.
pub const CHARACTER_CLASS: &str = "CharacterComponent";

/// The kernel's event for a possession starting or ending.
const POSSESSION_EVENT: &str = "PossessionChanged";

/// What each world's session has heard; by the world's address.
static SESSIONS: Mutex<Option<HashMap<usize, Session>>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn number(event: &DynEvent, index: usize) -> f32 {
    match event.fields.get(index) {
        Some(DynValue::F64(value)) => *value as f32,
        _ => 0.0,
    }
}

/// One world's session: the pawn the local player possesses, and what that
/// pawn has heard of the player's actions.
struct Session {
    /// Bits of the possessed pawn, `None` when nothing is possessed.
    pawn: Arc<Mutex<Option<u64>>>,
    _possession: Option<SyncSubscription>,
    listening: Option<Listening>,
}

/// What one pawn has heard.
struct Listening {
    pawn: u64,
    /// `Move`: x right, y forward.
    axis: Arc<Mutex<[f32; 2]>>,
    /// `Jump` was pressed; cleared when applied.
    jump: Arc<AtomicBool>,
    /// The events subscribed so far (an action the mapping does not have has
    /// no event to hear; it is looked for again until it shows up).
    heard: HashSet<&'static str>,
    subscriptions: Vec<SyncSubscription>,
}

impl Listening {
    fn new(pawn: u64) -> Self {
        Self {
            pawn,
            axis: Arc::new(Mutex::new([0.0; 2])),
            jump: Arc::new(AtomicBool::new(false)),
            heard: HashSet::new(),
            subscriptions: Vec::new(),
        }
    }

    /// Listen for the events of the actions the project has. They are
    /// declared when the session begins; one that is missing is not in the
    /// project, and costs a lookup a frame.
    fn listen(&mut self, hub: &EventHub) {
        let channel = Channel::Entity(self.pawn);
        let (heard, subscriptions) = (&mut self.heard, &mut self.subscriptions);
        let mut subscribe = |name: &'static str, handler: Box<dyn Fn(&DynEvent) + Send + Sync>| {
            if heard.contains(name) {
                return;
            }
            let Some(descriptor) = hub.descriptor_by_name(name) else {
                return;
            };
            subscriptions.push(hub.bus().subscribe_dyn(
                descriptor.id,
                SubscribeOptions::channel(channel),
                handler,
            ));
            heard.insert(name);
        };
        // x and y are fields 2 and 3 of an action's event.
        for name in [
            "input::Move.started",
            "input::Move.ongoing",
            "input::Move.triggered",
        ] {
            let axis = self.axis.clone();
            subscribe(
                name,
                Box::new(move |event| *lock(&axis) = [number(event, 2), number(event, 3)]),
            );
        }
        for name in ["input::Move.completed", "input::Move.canceled"] {
            let axis = self.axis.clone();
            subscribe(name, Box::new(move |_| *lock(&axis) = [0.0; 2]));
        }
        let jump = self.jump.clone();
        subscribe(
            "input::Jump.started",
            Box::new(move |_| jump.store(true, Ordering::SeqCst)),
        );
    }
}

fn has_character(world: &World, pawn: Entity) -> bool {
    attachments::instances(world, pawn).iter().any(|instance| {
        attachments::meta(world, *instance).is_some_and(|meta| meta.class_name == CHARACTER_CLASS)
    })
}

/// After the scripts: follow the possessed pawn, make sure it hears the
/// player, and hand what it heard to its character for the next frame's
/// movement.
pub fn drive(world: &mut World, hub: &EventHub, _resolve: ScriptVariableResolver) {
    let key = world as *const World as usize;
    let (pawn_bits, axis, jump) = {
        let mut guard = lock(&SESSIONS);
        let session = guard
            .get_or_insert_with(HashMap::new)
            .entry(key)
            .or_insert_with(|| {
                let pawn: Arc<Mutex<Option<u64>>> = Arc::default();
                let possession = hub.descriptor_by_name(POSSESSION_EVENT).map(|descriptor| {
                    let pawn = pawn.clone();
                    hub.bus().subscribe_dyn(
                        descriptor.id,
                        SubscribeOptions::default(),
                        move |event: &DynEvent| {
                            let possessed =
                                matches!(event.fields.get(2), Some(DynValue::Bool(true)));
                            let bits = match event.fields.get(1) {
                                Some(DynValue::U64(bits)) => Some(*bits),
                                _ => None,
                            };
                            *lock(&pawn) = if possessed { bits } else { None };
                        },
                    )
                });
                Session {
                    pawn,
                    _possession: possession,
                    listening: None,
                }
            });
        let Some(bits) = *lock(&session.pawn) else {
            session.listening = None;
            return;
        };
        if session.listening.as_ref().map(|l| l.pawn) != Some(bits) {
            session.listening = Some(Listening::new(bits));
        }
        let listening = session.listening.as_mut().expect("just set");
        listening.listen(hub);
        let axis = *lock(&listening.axis);
        (bits, axis, listening.jump.swap(false, Ordering::SeqCst))
    };
    let pawn = Entity::from_bits(pawn_bits);
    if !world.is_alive(pawn) || !has_character(world, pawn) {
        return;
    }
    let set = |world: &mut World, property: &str, value: serde_json::Value| {
        if let Err(error) = pulsar_world_registry::set_component_property(
            world,
            pawn,
            CHARACTER_CLASS,
            0,
            property,
            value,
        ) {
            tracing::warn!("gameplay: {CHARACTER_CLASS}.{property}: {error}");
        }
    };
    set(world, "move_input", serde_json::json!(axis));
    if jump {
        set(world, "jump", serde_json::json!(true));
    }
}

pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.add(TickPhaseRegistration {
        origin: p.origin(),
        name: "gameplay.player_input",
        before_systems: None,
        after_scripts: Some(drive),
    });
}
