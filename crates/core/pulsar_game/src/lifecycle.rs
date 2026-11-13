//! The component lifecycle, as game code sees it, and the native script actors
//! that run through it.
//!
//! [`Actor`], [`WorldActor`], [`ActorCx`] and the rest are the component
//! lifecycle traits of `pulsar_component_lifecycle` (`construct`, `begin_play`,
//! `tick`, `end_play`), re-exported here because game crates depend only on
//! `pulsar_game`. [`Actor`] is also `pulsar_game::Actor` and
//! `pulsar_game::prelude::Actor`.
//!
//! None of this is reflected: a lifecycle callback is a table entry, never a
//! Blueprint node (`pulsar_component_lifecycle`, "Not reachable from
//! Blueprints").
//!
//! # Native script actors
//!
//! A native script actor (a generated Rust Blueprint class, or a hand-written
//! one in a project's `scripts/` crate) is a boxed object that owns state and
//! needs the world in every callback. It is not a component, so it implements
//! [`NativeActor`] and runs through a component of its own:
//!
//! - the actor's **owner object** is an entity (what `TickLoop::register_native_actor`
//!   returns, stamped with a [`ScriptTag`](crate::scripts::ScriptTag));
//! - an **instance entity**, linked to the owner like any component instance
//!   (`ComponentOwner`), carries an [`ActorCarrier`] marker;
//! - the carrier's table ([`EngineActor`], class `NativeActorCarrier`) calls
//!   into the box, which lives in [`ActorBank`], a handle the session scope
//!   holds as a resource. The table is world-form, so the box can be taken out,
//!   called with `&mut World`, and put back, with no component borrow held.
//!
//! So an actor gets `begin_play` on the first frame the session ticks it,
//! `tick` every frame, and `end_play` when its instance is despawned or
//! disabled, **and when the session ends**, before the host restores a
//! snapshot. Hot reload re-expresses as "the same class on the same owner
//! re-attaches" (`scripts.rs`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

pub use pulsar_component_lifecycle::{
    Actor, ActorCx, ActorTable, ConstructInfo, ConstructReason, EngineActor, SessionKind,
    SessionScope, WorldActor,
};
use pulsar_scenedb::{Entity, World};
pub(crate) use pulsar_world_registry::pulsar_scene_model::ComponentOwner;

/// A boxed gameplay actor: state of its own and the live world in every
/// callback. Generated Rust Blueprint actors implement it
/// (`pulsar_script_codegen`).
///
/// `tick` is given the frame's time; `cx` is the lifecycle callback context
/// (event hub, session kind and resources). All three are optional.
///
/// Unlike [`Actor`] (a component's own methods, with no world), this is the
/// form for objects the framework cannot borrow out of the world: the owner
/// object is `entity`, and `end_play` has the world while it is still alive.
pub trait NativeActor: Send + 'static {
    /// The actor became live: the first frame its session ticks it, with the
    /// level in the world.
    fn begin_play(&mut self, _entity: Entity, _world: &mut World, _cx: &mut ActorCx<'_>) {}

    /// Every frame while live.
    fn tick(
        &mut self,
        _entity: Entity,
        _world: &mut World,
        _cx: &mut ActorCx<'_>,
        _delta_seconds: f32,
    ) {
    }

    /// The actor stopped being live: removed or disabled, or its session
    /// ended.
    fn end_play(&mut self, _entity: Entity, _world: &mut World, _cx: &mut ActorCx<'_>) {}
}

/// Run `f` with a lifecycle context of a throwaway headless session (an
/// empty event hub, no resources), to drive a [`NativeActor`] by hand: a
/// test, or a tool that calls one callback.
pub fn with_scratch_cx<R>(f: impl FnOnce(&mut ActorCx<'_>) -> R) -> R {
    let events = pulsar_events::EventHub::new();
    let mut session = SessionScope::new(SessionKind::Headless);
    let mut cx = ActorCx::new(Entity::DANGLING, None, &events, &mut session);
    f(&mut cx)
}

/// The component on a native actor's instance entity. It carries only the
/// actor's type path: the actor itself is in the session's [`ActorBank`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorCarrier {
    pub type_path: String,
}

/// The class name the carrier's table registers under.
pub const CARRIER_CLASS: &str = "NativeActorCarrier";

/// Every live native actor, by its instance entity. Clones share one map: the
/// `TickLoop` keeps a handle and puts one in the session scope (a session end
/// or a replaced world drops the scope's resources, never the actors).
#[derive(Clone, Default)]
pub(crate) struct ActorBank(Arc<Mutex<HashMap<Entity, Box<dyn NativeActor>>>>);

impl ActorBank {
    pub(crate) fn insert(&self, instance: Entity, actor: Box<dyn NativeActor>) {
        self.lock().insert(instance, actor);
    }

    /// Take the actor out for a callback (no lock is held while it runs).
    fn take(&self, instance: Entity) -> Option<Box<dyn NativeActor>> {
        self.lock().remove(&instance)
    }

    /// Drop every actor. Called while the code that made them is still loaded.
    pub(crate) fn clear(&self) {
        // Dropped after the lock is released: an actor's Drop may do anything.
        let actors = std::mem::take(&mut *self.lock());
        drop(actors);
    }

    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<Entity, Box<dyn NativeActor>>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Run `call` on the actor behind `instance`, if the session has it.
fn with_actor(
    world: &mut World,
    cx: &mut ActorCx<'_>,
    instance: Entity,
    keep: bool,
    call: impl FnOnce(&mut dyn NativeActor, Entity, &mut World, &mut ActorCx<'_>),
) {
    let Some(bank) = cx.session.resource::<ActorBank>().cloned() else {
        return;
    };
    let Some(mut actor) = bank.take(instance) else {
        return;
    };
    let owner = cx.entity;
    call(&mut *actor, owner, world, cx);
    if keep {
        bank.insert(instance, actor);
    }
}

impl WorldActor for ActorCarrier {
    fn begin_play(world: &mut World, cx: &mut ActorCx<'_>, instance: Entity) {
        with_actor(world, cx, instance, true, |actor, owner, world, cx| {
            actor.begin_play(owner, world, cx);
        });
    }

    fn tick(world: &mut World, cx: &mut ActorCx<'_>, instance: Entity, delta_seconds: f32) {
        with_actor(world, cx, instance, true, |actor, owner, world, cx| {
            actor.tick(owner, world, cx, delta_seconds);
        });
    }

    fn end_play(world: &mut World, cx: &mut ActorCx<'_>) {
        let Some(instance) = cx.instance else {
            return;
        };
        // The actor ends for good: it is not put back.
        with_actor(world, cx, instance, false, |actor, owner, world, cx| {
            actor.end_play(owner, world, cx);
        });
    }
}

fn carrier_table() -> ActorTable {
    ActorTable {
        // Nothing constructs in the editor: an actor is a play-time object.
        construct: None,
        ..ActorTable::of_world::<ActorCarrier>()
    }
}

inventory::submit! {
    EngineActor {
        class_name: CARRIER_CLASS,
        table: carrier_table,
    }
}
