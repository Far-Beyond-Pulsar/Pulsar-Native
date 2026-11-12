//! The component lifecycle capability registry (L1).
//!
//! A component class that wants lifecycle events implements [`Actor`] (or
//! [`WorldActor`]) and registers an [`ActorTable`] next to its class:
//! `ClassSpec { actor: Some(ActorTable::of::<T>()), .. }`
//! (`engine_backend::scene::class_spec`). The registry stores one
//! [`ActorRegistration`] per class, gated by the class's plugin like the
//! class and its systems, and `pulsar_world_registry::tick_live_components`
//! drives it in the `pulsar.components` step.
//!
//! # Not reachable from Blueprints
//!
//! This crate depends on SceneDB, the event hub and the plugin gate, and on
//! nothing that reflects a class or runs a script (`pulsar_reflection`,
//! `pulsar_script_vm`; the `crate_edges` test pins it). The tables are a
//! registry of their own, never reflected methods, so `begin_play` is not a
//! Blueprint node by construction. A class's reflected methods must also not
//! use a lifecycle name: the audit
//! `pulsar_world_registry::audit::find_leaked_lifecycle_methods` fails on it,
//! because the reflection registry has no hidden flag.
//!
//! # Events
//!
//! | event | when | in this phase |
//! |---|---|---|
//! | `construct` | the component is added or changed | declared, never delivered |
//! | `begin_play` | an enabled instance is first seen by a running session | delivered |
//! | `tick` | each frame, for every live enabled instance | delivered |
//! | `end_play` | disabled, removed, owner despawned or session end | delivered, entity-only |
//!
//! `end_play` never sees the component: SceneDB has already dropped it (its
//! change journal carries no payload), so it receives the owner and the
//! session's services only. State a class needs at the end belongs to
//! session resources keyed by owner.
//!
//! # Two forms
//!
//! - [`Actor`]: `&mut self` callbacks. The framework holds the component's
//!   row borrowed from the world, so the callback gets an [`ActorCx`] and no
//!   `World`: it can never re-enter a world borrow.
//! - [`WorldActor`]: callbacks that get `&mut World` and the instance entity
//!   and hold no component borrow, for classes that must see siblings
//!   (physics). The callback fetches its own component.
//!
//! A class picks one form per event: [`ActorTable::of`] and
//! [`ActorTable::of_world`] fill every event from one trait, and the fields
//! are public to mix them (`ActorTable { tick: ActorTable::of_world::<W>().tick,
//! ..ActorTable::of::<T>() }`).
//!
//! How others do it: Unreal's `UActorComponent::BeginPlay/TickComponent/
//! EndPlay` and `OnRegister`; Unity's `Awake/Start/Update/OnDestroy`;
//! Godot's `_ready/_process/_exit_tree`. Ours keeps Unreal's names and makes
//! the world-less form the default, since the borrow checker is the guard
//! Unreal's discipline is not.

use pulsar_events::EventHub;
use pulsar_scenedb::{component_id, Component, ComponentId, Entity, World};

/// What a lifecycle callback runs with.
pub struct ActorCx<'a> {
    /// The owner object: the entity the component is attached to, whose event
    /// channel the instance uses.
    pub entity: Entity,
    /// The component-instance entity. `None` in `end_play`: the instance is
    /// already gone.
    pub instance: Option<Entity>,
    /// The session's event hub.
    pub events: &'a EventHub,
}

impl<'a> ActorCx<'a> {
    pub fn new(entity: Entity, instance: Option<Entity>, events: &'a EventHub) -> Self {
        Self {
            entity,
            instance,
            events,
        }
    }
}

/// A component's lifecycle, in the form that borrows the component and
/// gives no world (module docs).
pub trait Actor: Component + Sized {
    /// Whether this instance is live. A disabled one is ended like a removed
    /// one (the owner link's own enabled flag always applies too).
    fn enabled(&self) -> bool {
        true
    }

    /// The component was added or changed. Declared for the editor pump of a
    /// later phase; nothing calls it yet.
    fn construct(&mut self, _cx: &mut ActorCx<'_>) {}

    /// The instance became live in a running session.
    fn begin_play(&mut self, _cx: &mut ActorCx<'_>) {}

    /// Every frame while live.
    fn tick(&mut self, _cx: &mut ActorCx<'_>, _delta_seconds: f32) {}

    /// The instance stopped being live. Owner-only: `cx.instance` is `None`
    /// and the component cannot be read.
    fn end_play(_cx: &mut ActorCx<'_>) {}
}

/// A component's lifecycle in the form that gets the world and no component
/// borrow (module docs). `instance` is the component-instance entity.
pub trait WorldActor: Component + Sized {
    /// As [`Actor::enabled`].
    fn enabled(&self) -> bool {
        true
    }

    fn construct(_world: &mut World, _cx: &mut ActorCx<'_>, _instance: Entity) {}

    fn begin_play(_world: &mut World, _cx: &mut ActorCx<'_>, _instance: Entity) {}

    fn tick(_world: &mut World, _cx: &mut ActorCx<'_>, _instance: Entity, _delta_seconds: f32) {}

    /// Owner-only, as [`Actor::end_play`].
    fn end_play(_cx: &mut ActorCx<'_>) {}
}

/// `construct`/`begin_play` as the framework calls them: the instance
/// entity names the component.
pub type InstanceFn = fn(&mut World, &mut ActorCx<'_>, Entity);
pub type TickFn = fn(&mut World, &mut ActorCx<'_>, Entity, f32);
pub type EndFn = fn(&mut ActorCx<'_>);

/// A class's lifecycle: function pointers shaped by the framework, so a table
/// crosses the plugin library boundary like the other registries.
#[derive(Clone, Copy)]
pub struct ActorTable {
    /// The class's SceneDB component id.
    pub component_type: fn() -> ComponentId,
    /// The Rust type's name: a deterministic order, never an id.
    pub type_name: &'static str,
    /// Push the class's instance entities that are enabled
    /// ([`Actor::enabled`]).
    pub instances: fn(&mut World, &mut Vec<Entity>),
    pub construct: Option<InstanceFn>,
    pub begin_play: Option<InstanceFn>,
    pub tick: Option<TickFn>,
    pub end_play: Option<EndFn>,
}

impl ActorTable {
    /// Every event through [`Actor`].
    pub fn of<T: Actor>() -> Self {
        fn instances<T: Actor>(world: &mut World, out: &mut Vec<Entity>) {
            out.extend(
                world
                    .query::<&T>()
                    .filter(|(_, component)| component.enabled())
                    .map(|(entity, _)| entity),
            );
        }
        fn construct<T: Actor>(world: &mut World, cx: &mut ActorCx<'_>, instance: Entity) {
            if let Some(mut component) = world.get_mut::<T>(instance) {
                component.construct(cx);
            }
        }
        fn begin_play<T: Actor>(world: &mut World, cx: &mut ActorCx<'_>, instance: Entity) {
            if let Some(mut component) = world.get_mut::<T>(instance) {
                component.begin_play(cx);
            }
        }
        fn tick<T: Actor>(world: &mut World, cx: &mut ActorCx<'_>, instance: Entity, dt: f32) {
            if let Some(mut component) = world.get_mut::<T>(instance) {
                component.tick(cx, dt);
            }
        }
        Self {
            component_type: component_id::<T>,
            type_name: std::any::type_name::<T>(),
            instances: instances::<T>,
            construct: Some(construct::<T>),
            begin_play: Some(begin_play::<T>),
            tick: Some(tick::<T>),
            end_play: Some(T::end_play),
        }
    }

    /// Every event through [`WorldActor`].
    pub fn of_world<T: WorldActor>() -> Self {
        fn instances<T: WorldActor>(world: &mut World, out: &mut Vec<Entity>) {
            out.extend(
                world
                    .query::<&T>()
                    .filter(|(_, component)| component.enabled())
                    .map(|(entity, _)| entity),
            );
        }
        Self {
            component_type: component_id::<T>,
            type_name: std::any::type_name::<T>(),
            instances: instances::<T>,
            construct: Some(T::construct),
            begin_play: Some(T::begin_play),
            tick: Some(T::tick),
            end_play: Some(T::end_play),
        }
    }
}

/// A registered table, with the class that gates it. Added by
/// `Components::class` (`ClassSpec::actor`) through the plugin's builder.
#[derive(Clone, Copy)]
pub struct ActorRegistration {
    /// The plugin root that registered the class.
    pub origin: &'static str,
    /// The class whose registration gates it.
    pub class_name: &'static str,
    pub table: ActorTable,
}

impl pulsar_plugins::Contribution for ActorRegistration {
    const CATEGORY: &'static str = "component.lifecycle";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.class_name.to_owned()
    }

    fn label(&self) -> String {
        let events: Vec<&str> = [
            ("construct", self.table.construct.is_some()),
            ("begin_play", self.table.begin_play.is_some()),
            ("tick", self.table.tick.is_some()),
            ("end_play", self.table.end_play.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect();
        format!("{} ({})", self.class_name, events.join(", "))
    }
}

// A plugin library's tables are imported as this type
// (`pulsar_plugins::library`).
pulsar_plugins::importer!(ActorRegistration);

/// The names a table reserves: a reflected method of any class must not use
/// one, or it would be a Blueprint-callable native
/// (`pulsar_world_registry::audit`).
pub const RESERVED_METHODS: &[&str] = &["construct", "begin_play", "tick", "end_play"];

/// Registrations ordered for lookup: by component id (binary search) and by
/// type name (the deterministic frame order).
pub struct ActorIndex {
    by_type_name: Vec<&'static ActorRegistration>,
    by_component: Vec<(ComponentId, &'static ActorRegistration)>,
}

impl ActorIndex {
    pub fn new(registrations: &[&'static ActorRegistration]) -> Self {
        let mut by_type_name = registrations.to_vec();
        by_type_name.sort_by_key(|r| (r.table.type_name, r.class_name));
        let mut by_component: Vec<_> = registrations
            .iter()
            .map(|r| ((r.table.component_type)(), *r))
            .collect();
        by_component.sort_by_key(|(id, r)| (*id, r.class_name));
        Self {
            by_type_name,
            by_component,
        }
    }

    /// The table of the class with component id `id`.
    pub fn get(&self, id: ComponentId) -> Option<&'static ActorRegistration> {
        let at = self.by_component.partition_point(|(other, _)| *other < id);
        self.by_component
            .get(at)
            .filter(|(other, _)| *other == id)
            .map(|(_, r)| *r)
    }

    /// Every registration, in the frame's order (type name).
    pub fn iter(&self) -> impl Iterator<Item = &'static ActorRegistration> + '_ {
        self.by_type_name.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    struct B;
    impl Actor for A {}
    impl WorldActor for B {}

    fn leak(class_name: &'static str, table: ActorTable) -> &'static ActorRegistration {
        Box::leak(Box::new(ActorRegistration {
            origin: "test",
            class_name,
            table,
        }))
    }

    #[test]
    fn lookup_finds_by_component_id_and_orders_by_type_name() {
        let b = leak("B", ActorTable::of_world::<B>());
        let a = leak("A", ActorTable::of::<A>());
        let index = ActorIndex::new(&[b, a]);
        assert_eq!(index.get(component_id::<A>()).unwrap().class_name, "A");
        assert_eq!(index.get(component_id::<B>()).unwrap().class_name, "B");
        assert!(index.get(component_id::<u8>()).is_none());
        let order: Vec<_> = index.iter().map(|r| r.class_name).collect();
        assert_eq!(order, ["A", "B"]);
    }

    /// The module docs' promise: nothing here reaches reflection or the
    /// script VM, so no lifecycle function can be a Blueprint native.
    #[test]
    fn crate_edges() {
        let manifest = include_str!("../Cargo.toml");
        let dependencies: Vec<&str> = manifest
            .lines()
            .skip_while(|line| line.trim() != "[dependencies]")
            .skip(1)
            .take_while(|line| !line.trim_start().starts_with('['))
            .filter_map(|line| line.split('=').next())
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.starts_with('#'))
            .collect();
        assert!(!dependencies.is_empty());
        for forbidden in [
            "pulsar_reflection",
            "pulsar_script_vm",
            "pulsar_script_runtime",
            "pulsar_world_registry",
            "pulsar_scene_model",
        ] {
            assert!(
                !dependencies.contains(&forbidden),
                "pulsar_component_lifecycle must not depend on {forbidden}"
            );
        }
    }
}
