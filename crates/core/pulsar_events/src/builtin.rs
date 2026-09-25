//! Built-in engine events.
//!
//! Every type here is a `#[pulsar_event(dynamic)]` Gamma event, so Rust
//! code publishes and subscribes to it as a struct while scripts, Blueprint
//! and plugins see its [`EventDescriptor`] and receive it as a `DynEvent`.
//! [`EventHub::new`](crate::EventHub::new) registers all of them.
//!
//! # Conventions
//!
//! - Names are plain `PascalCase` (`Hit`, `LevelLoaded`); script-declared
//!   events are named by their frontend (the Blueprint compiler uses
//!   `<Class>.<Event>`), so they cannot collide with these.
//! - A `u64` field is always an **entity handle** (`Entity::bits()`);
//!   scripts see it as `entity`. Integers that are not entities (key codes,
//!   timer ids) are `i64`.
//! - Events about one object are published on that object's entity
//!   channel ([`entity_channel`](crate::entity_channel)); world-wide ones on
//!   the global channel. The table lists each event's channel.
//!
//! | Event | Channel | Published by |
//! |---|---|---|
//! | [`LevelLoaded`] | global | the script driver, once, after the level's instances started |
//! | [`BeginPlay`] / [`EndPlay`] | the instance's entity | the script driver, after an instance's `begin_play` / `end_play` |
//! | [`EntitySpawned`] / [`EntityDestroyed`] | global | the script driver (`world::spawn` / `world::destroy`, objects removed from the world) |
//! | [`KeyDown`] / [`KeyUp`], [`MouseButtonDown`] / [`MouseButtonUp`] | global | the game window / Play-in-Editor input forwarding |
//! | [`Hit`], [`BeginOverlap`], [`EndOverlap`] | the entity hit / overlapping | physics (see below) |
//! | [`TimerFired`] | the timer owner's entity (global for unbound scripts) | `timer::set` timers |
//! | [`Damage`] | the damaged entity | `game::apply_damage` |
//!
//! Physics: `pulsar_physics` does not simulate or report contacts yet, so
//! nothing publishes `Hit` / `BeginOverlap` / `EndOverlap` today.
//! TODO(Pulsar-Native#924): publish them from the physics step (on the
//! entity channel of each body involved, one event per body) once
//! `pulsar_physics` exposes contact and overlap events.

use gamma::{Event, EventDescriptor, pulsar_event};

use crate::hub::EventCategory;

/// The level finished loading and its objects' scripts have started.
#[pulsar_event(dynamic, name = "LevelLoaded", crate = gamma)]
#[derive(Clone, Debug, PartialEq)]
pub struct LevelLoaded {
    /// The level file (empty when unknown, e.g. a world built in code).
    pub level: String,
}

/// An instance's `begin_play` ran.
#[pulsar_event(dynamic, name = "BeginPlay", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeginPlay {
    pub entity: u64,
}

/// An instance's `end_play` ran (it is being destroyed or the game stops).
#[pulsar_event(dynamic, name = "EndPlay", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EndPlay {
    pub entity: u64,
}

/// An object was spawned at runtime.
#[pulsar_event(dynamic, name = "EntitySpawned", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntitySpawned {
    pub entity: u64,
}

/// An object was removed from the world.
#[pulsar_event(dynamic, name = "EntityDestroyed", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityDestroyed {
    pub entity: u64,
}

/// A key went down. `key` is the platform-independent key code the input
/// source forwards (the Play-in-Editor ABI's virtual key code, or winit's
/// `KeyCode` discriminant in a standalone window).
#[pulsar_event(dynamic, name = "KeyDown", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyDown {
    pub key: i64,
}

/// A key went up.
#[pulsar_event(dynamic, name = "KeyUp", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyUp {
    pub key: i64,
}

/// A mouse button went down (0 = left, 1 = right, 2 = middle).
#[pulsar_event(dynamic, name = "MouseButtonDown", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseButtonDown {
    pub button: i64,
}

/// A mouse button went up.
#[pulsar_event(dynamic, name = "MouseButtonUp", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseButtonUp {
    pub button: i64,
}

/// `entity` hit `other` (blocking contact). Published on `entity`'s channel.
#[pulsar_event(dynamic, name = "Hit", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub entity: u64,
    pub other: u64,
    /// Magnitude of the contact impulse.
    pub impulse: f64,
}

/// `entity` started overlapping `other`. Published on `entity`'s channel.
#[pulsar_event(dynamic, name = "BeginOverlap", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeginOverlap {
    pub entity: u64,
    pub other: u64,
}

/// `entity` stopped overlapping `other`. Published on `entity`'s channel.
#[pulsar_event(dynamic, name = "EndOverlap", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EndOverlap {
    pub entity: u64,
    pub other: u64,
}

/// A timer set with `timer::set` elapsed.
#[pulsar_event(dynamic, name = "TimerFired", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimerFired {
    /// The id `timer::set` returned.
    pub timer: i64,
}

/// `target` took `amount` damage from `instigator` (0 when unknown).
/// Published on `target`'s channel.
#[pulsar_event(dynamic, name = "Damage", crate = gamma)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Damage {
    pub target: u64,
    pub amount: f64,
    pub instigator: u64,
}

/// Every built-in event with its palette category, in a stable order.
pub fn builtin_events() -> Vec<(EventDescriptor, EventCategory)> {
    fn d<T: Event>(category: EventCategory) -> (EventDescriptor, EventCategory) {
        (T::descriptor().expect("built-in events are reflected"), category)
    }
    use EventCategory as C;
    vec![
        d::<LevelLoaded>(C::Lifecycle),
        d::<BeginPlay>(C::Lifecycle),
        d::<EndPlay>(C::Lifecycle),
        d::<EntitySpawned>(C::World),
        d::<EntityDestroyed>(C::World),
        d::<KeyDown>(C::Input),
        d::<KeyUp>(C::Input),
        d::<MouseButtonDown>(C::Input),
        d::<MouseButtonUp>(C::Input),
        d::<Hit>(C::Physics),
        d::<BeginOverlap>(C::Physics),
        d::<EndOverlap>(C::Physics),
        d::<TimerFired>(C::Gameplay),
        d::<Damage>(C::Gameplay),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamma::FieldType;

    #[test]
    fn descriptors_use_script_friendly_fields() {
        let events = builtin_events();
        assert_eq!(events.len(), 14);
        let hit = &events.iter().find(|(d, _)| d.name == "Hit").unwrap().0;
        assert_eq!(hit.id, Hit::stable_type_id());
        assert_eq!(
            hit.fields,
            vec![
                ("entity".to_owned(), FieldType::U64),
                ("other".to_owned(), FieldType::U64),
                ("impulse".to_owned(), FieldType::F64)
            ]
        );
        let names: std::collections::HashSet<_> = events.iter().map(|(d, _)| &d.name).collect();
        assert_eq!(names.len(), events.len(), "names are unique");
    }
}
