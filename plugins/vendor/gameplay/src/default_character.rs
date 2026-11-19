//! The default player character, and the bridge that lets the player move it.
//!
//! When a level assigns no Player Character class, the session spawns this
//! natively ([`spawn`], a `RoleDefaultRegistration`): a scene object with a
//! `CharacterComponent` (which places its controller and input with it) and a
//! `CameraComponent`, so the possessed pawn has a body and a view.
//!
//! The input plugin's possession hook puts the level's mapping to work on
//! the pawn (the level's `player_input_mapping` World Setting), and its events
//! (`input::Move.*`, `input::Jump.started`) arrive on the pawn's entity
//! channel. [`bridge`] hears them there and writes the character's
//! `move_input` and `jump`, which the character's own system turns into
//! movement: `Move` is x right, y forward, as the example mapping's WASD
//! and stick bindings produce it.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine_backend::scene::game_hooks::{
    RoleDefaultRegistration, ScriptVariableResolver, TickPhaseRegistration,
};
use engine_backend::scene::{ObjectType, SceneWorldExt, SpawnObject};
use pulsar_events::gamma::{Channel, DynEvent, DynValue, SubscribeOptions, SyncSubscription};
use pulsar_events::EventHub;
use pulsar_scenedb::{Entity, World};
use pulsar_traits::builtin::PLAYER_CHARACTER_ROLE;
use pulsar_world_registry::pulsar_scene_model::attachments::NewInstance;
use pulsar_world_registry::{place_component, ComponentPayload};

/// The character plugin's class: the pawn's body and what drives it.
pub const CHARACTER_CLASS: &str = "CharacterComponent";
/// The engine's camera class: the pawn's view.
pub const CAMERA_CLASS: &str = "CameraComponent";

/// The input actions the character is driven by, by the name the mapping's
/// actions go by (`input::<Action>.<phase>`).
pub const MOVE_ACTION: &str = "Move";
pub const JUMP_ACTION: &str = "Jump";

/// The default pawn of each world (by the world's address), so the bridge
/// knows which entity is the plugin's.
static PAWNS: Mutex<Vec<(usize, Entity)>> = Mutex::new(Vec::new());
/// What each world's pawn has heard.
static BRIDGES: Mutex<Option<HashMap<usize, Bridge>>> = Mutex::new(None);

fn world_key(world: &World) -> usize {
    world as *const World as usize
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Spawn the default player character: an object with a character and a
/// camera. The character places its controller and input with it.
pub fn spawn(world: &mut World, _project_root: &Path) -> Result<Entity, String> {
    let mut spec = SpawnObject::new("Player Character");
    spec.object_type = ObjectType::Empty;
    let pawn = world
        .spawn_object(spec)
        .map_err(|error| format!("the object could not be created: {error}"))?;
    for class in [CHARACTER_CLASS, CAMERA_CLASS] {
        place_component(
            world,
            pawn,
            NewInstance::new(class),
            ComponentPayload::Default,
        )
        .map_err(|error| {
            world.despawn_tree(pawn);
            format!("`{class}` could not be placed: {error}")
        })?;
    }
    let key = world_key(world);
    let mut pawns = lock(&PAWNS);
    pawns.retain(|(known, _)| *known != key);
    pawns.push((key, pawn));
    Ok(pawn)
}

/// What the pawn has heard of the player's actions since the last frame.
struct Bridge {
    pawn: Entity,
    /// `Move`: x right, y forward.
    axis: Arc<Mutex<[f32; 2]>>,
    /// `Jump` was pressed; cleared when applied.
    jump: Arc<AtomicBool>,
    /// The events subscribed so far (an action the mapping does not have
    /// has no event to hear).
    heard: HashSet<&'static str>,
    _subscriptions: Vec<SyncSubscription>,
}

fn number(event: &DynEvent, index: usize) -> f32 {
    match event.fields.get(index) {
        Some(DynValue::F64(value)) => *value as f32,
        _ => 0.0,
    }
}

impl Bridge {
    fn new(pawn: Entity) -> Self {
        Self {
            pawn,
            axis: Arc::new(Mutex::new([0.0; 2])),
            jump: Arc::new(AtomicBool::new(false)),
            heard: HashSet::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// Listen for the events of the actions the mapping has. The project's
    /// action events are declared when the session begins, so one that is
    /// missing now is not in the project; it is looked for again next frame
    /// only until it shows up, and costs a lookup.
    fn listen(&mut self, hub: &EventHub) {
        let channel = Channel::Entity(self.pawn.bits());
        let mut subscribe = |name: &'static str, handler: Box<dyn Fn(&DynEvent) + Send + Sync>| {
            if self.heard.contains(name) {
                return;
            }
            let Some(descriptor) = hub.descriptor_by_name(name) else {
                return;
            };
            self._subscriptions.push(hub.bus().subscribe_dyn(
                descriptor.id,
                SubscribeOptions::channel(channel),
                handler,
            ));
            self.heard.insert(name);
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

/// After the scripts: make sure the pawn hears the player, and hand what it
/// heard to its character for the next frame's movement.
pub fn bridge(world: &mut World, hub: &EventHub, _resolve: ScriptVariableResolver) {
    let key = world_key(world);
    let Some(pawn) = lock(&PAWNS)
        .iter()
        .find(|(known, _)| *known == key)
        .map(|(_, pawn)| *pawn)
    else {
        return;
    };
    if !world.is_alive(pawn) {
        lock(&PAWNS).retain(|(known, _)| *known != key);
        if let Some(bridges) = lock(&BRIDGES).as_mut() {
            bridges.remove(&key);
        }
        return;
    }
    let (axis, jump) = {
        let mut guard = lock(&BRIDGES);
        let bridge = guard
            .get_or_insert_with(HashMap::new)
            .entry(key)
            .or_insert_with(|| Bridge::new(pawn));
        bridge.listen(hub);
        let axis = *lock(&bridge.axis);
        (axis, bridge.jump.swap(false, Ordering::SeqCst))
    };
    let set = |world: &mut World, property: &str, value: serde_json::Value| {
        if let Err(error) = pulsar_world_registry::set_component_property(
            world,
            pawn,
            CHARACTER_CLASS,
            0,
            property,
            value,
        ) {
            tracing::warn!("default character: {CHARACTER_CLASS}.{property}: {error}");
        }
    };
    set(world, "move_input", serde_json::json!(axis));
    if jump {
        set(world, "jump", serde_json::json!(true));
    }
}

pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.add(RoleDefaultRegistration {
        origin: p.origin(),
        name: "gameplay.default_player_character",
        role: PLAYER_CHARACTER_ROLE,
        spawn,
    });
    p.add(TickPhaseRegistration {
        origin: p.origin(),
        name: "gameplay.player_input",
        before_systems: None,
        after_scripts: Some(bridge),
    });
}
