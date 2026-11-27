//! The game instance and the level boundaries of a session.
//!
//! **The game instance** is the project's own object (a Blueprint class
//! implementing `pulsar.GameInstance`, picked in Project Settings): spawned
//! when the engine starts, before any level loads, and alive until the
//! session ends. Its script starts at once ([`ScriptDriver::start_game_instance`]:
//! `begin_play`, then the trait's `on_init`). It is not a level role: World
//! Settings does not list it and a level switch does not end it. Its object,
//! its objects' children and their components (widgets) are the session's;
//! everything else in the world is the level's, objects it spawns at the
//! root included (spawn a child of it to keep one across levels).
//!
//! **Levels.** [`ScriptDriver::end_level`] ends the level's part of the
//! session (possession, the game mode, the level's scripts, the roles);
//! [`ScriptDriver::begin_level`] starts the next one, whose roles start at the
//! next script phase, followed by the game instance's `on_level_loaded`.
//! While no level is in the world, [`ScriptDriver::hold_level_roles`] keeps
//! the roles from starting.

use std::collections::HashSet;

use engine_backend::scene::SceneWorldExt;
use pulsar_events::builtin::BeginPlay;
use pulsar_events::entity_channel;
use pulsar_scenedb::{Entity, World};
use pulsar_traits::builtin::{GAME_INSTANCE_TRAIT_ID, GAME_MODE_ROLE, GAME_MODE_TRAIT_ID};

use super::world_commands::discard_commands;
use super::{DriverReport, ScriptDriver, MAX_COMMAND_ROUNDS};
use crate::scripting::commands::CommandScope;
use crate::scripting::traits::TraitCheck;

/// Marks the game instance's object (what `game::instance` finds).
#[derive(Clone, Copy, Debug)]
pub struct GameInstanceEntity;

/// The game instance object's StableId: one per world, never a level's.
pub const GAME_INSTANCE_STABLE_ID: &str = "pulsar_game_instance";

impl ScriptDriver {
    /// The game instance's object, while one runs.
    pub fn game_instance(&self) -> Option<Entity> {
        self.game_instance
    }

    /// The host requests scripts made since the last call, in order.
    pub fn take_host_requests(&mut self) -> Vec<crate::game_control::HostRequest> {
        std::mem::take(&mut self.host_requests)
    }

    /// The scene objects the session keeps across levels: the game instance's
    /// object and everything under it.
    pub fn game_instance_objects(&self, world: &World) -> HashSet<Entity> {
        let mut objects = HashSet::new();
        let mut next: Vec<Entity> = self
            .game_instance
            .into_iter()
            .filter(|entity| world.is_alive(*entity))
            .collect();
        while let Some(object) = next.pop() {
            if objects.insert(object) {
                next.extend(world.children_of(Some(object)));
            }
        }
        objects
    }

    /// Spawn the game instance of `class` (a class folder or asset path, a
    /// GUID or a name), start its script and run `begin_play` and `on_init`.
    /// A game instance a previous session left in the world (a hot reload) is
    /// removed first.
    pub fn start_game_instance(&mut self, world: &mut World, class: &str) -> DriverReport {
        let mut report = DriverReport::default();
        let leftovers: Vec<Entity> = world
            .query::<&GameInstanceEntity>()
            .map(|(entity, _)| entity)
            .filter(|entity| Some(*entity) != self.game_instance)
            .collect();
        for entity in leftovers {
            tracing::info!("Removing a game instance a previous session left in the world");
            world.despawn_tree(entity);
        }
        if self.game_instance.is_some() {
            return report;
        }
        let Some(entry) = self.resolve_class_path(class) else {
            let message = format!("Game instance class '{class}' is not in this project");
            tracing::error!("{message}");
            report.failures.push(message);
            return report;
        };
        let mut scope = CommandScope::begin();
        let entity = world.spawn();
        self.spawn_instance(
            world,
            entity,
            entry.id.as_str(),
            Some(GAME_INSTANCE_STABLE_ID.to_owned()),
            None,
            [0.0; 3],
            &mut report,
        );
        if !world.is_alive(entity) {
            discard_commands(world, scope.finish());
            return report;
        }
        world.insert(entity, GameInstanceEntity);
        self.game_instance = Some(entity);
        self.start_now(world, entity, &mut report);
        report
            .script_errors
            .extend(self.runtime.dispatch_pending_begin_play(world));
        if self.instance_of(entity).is_none() {
            let message = format!(
                "Game instance class '{}' has no compiled script",
                entry.name
            );
            tracing::error!("{message}");
            report.failures.push(message);
        } else {
            self.publish(
                entity_channel(entity.bits()),
                BeginPlay {
                    entity: entity.bits(),
                },
            );
            if self.entity_trait_check(world, entity, GAME_INSTANCE_TRAIT_ID) == TraitCheck::DoesNot
            {
                let message = format!(
                    "Game instance class '{}' does not implement the Game Instance trait",
                    entry.name
                );
                tracing::warn!("{message}");
                report.failures.push(message);
            }
            self.call_trait(
                world,
                entity,
                GAME_INSTANCE_TRAIT_ID,
                "on_init",
                &[],
                &mut report,
            );
        }
        for _ in 0..MAX_COMMAND_ROUNDS {
            let commands = scope.take();
            if commands.is_empty() {
                break;
            }
            self.apply_commands(world, commands, &mut report);
        }
        discard_commands(world, scope.finish());
        tracing::info!(class = %entry.name, "Game instance started");
        report
    }

    /// Keep the level roles (and the level announcement) from starting until
    /// a level is in the world ([`Self::begin_level`]).
    pub fn hold_level_roles(&mut self) {
        self.roles_held = true;
    }

    /// The level `name` is in the world (its roles set already,
    /// [`Self::set_world_roles`]): its roles start at the next script phase,
    /// `LevelLoaded` is announced and the game instance hears
    /// `on_level_loaded`.
    pub fn begin_level(&mut self, name: String) {
        self.level = name;
        self.roles_held = false;
        self.level_loaded_pending = true;
        if let Some(events) = self.events.as_mut() {
            events.reset_level_announcement();
        }
    }

    /// Tell the game instance the level that just started is loaded.
    pub(super) fn notify_level_loaded(&mut self, world: &mut World, report: &mut DriverReport) {
        if !std::mem::take(&mut self.level_loaded_pending) {
            return;
        }
        let Some(entity) = self.game_instance.filter(|e| world.is_alive(*e)) else {
            return;
        };
        let level = pulsar_script_vm::Value::Str(self.level.as_str().into());
        self.call_trait(
            world,
            entity,
            GAME_INSTANCE_TRAIT_ID,
            "on_level_loaded",
            &[level],
            report,
        );
    }

    /// End the level's part of the session, with its objects still in the
    /// world: possession ends, the game mode hears `on_game_end`, every
    /// script on a level object gets `end_play` (what it asks of the world
    /// now is dropped), the role entities go. The game instance's scripts and
    /// the global scripts go on. The caller ends the level's components and
    /// removes its objects.
    pub fn end_level(&mut self, world: &mut World) -> DriverReport {
        profiling::profile_scope!("Script::end_level");
        let mut report = DriverReport::default();
        let scope = CommandScope::begin();
        self.end_possession(world, &mut report);
        if let Some(mode) = self.role_entity(GAME_MODE_ROLE) {
            self.call_trait(
                world,
                mode,
                GAME_MODE_TRAIT_ID,
                "on_game_end",
                &[],
                &mut report,
            );
        }
        if self.roles_started {
            self.publish(
                pulsar_events::gamma::Channel::Global,
                pulsar_events::builtin::GameEnded {
                    level: self.level.clone(),
                },
            );
        }
        let keep = self.game_instance_objects(world);
        let leaving: Vec<Entity> = self
            .bound_instances()
            .into_iter()
            .map(|(entity, _)| entity)
            .filter(|entity| !keep.contains(entity))
            .collect();
        for entity in leaving {
            self.stop(world, entity, &mut report);
        }
        self.tracked.retain(|entity, _| keep.contains(entity));
        discard_commands(world, scope.finish());
        self.despawn_role_entities(world);
        self.roles_started = false;
        self.roles_held = true;
        self.game_began_pending = false;
        self.level_loaded_pending = false;
        self.noted.clear();
        self.level = String::new();
        if let Some(events) = self.events.as_mut() {
            events.reset_level_announcement();
        }
        report
    }

    /// The session ends: the game instance hears `on_shutdown` with its
    /// objects and components still live, then every script on them gets
    /// `end_play`. Call it after the level ended.
    pub fn shutdown_game_instance(&mut self, world: &mut World) -> DriverReport {
        let mut report = DriverReport::default();
        let Some(entity) = self.game_instance.filter(|e| world.is_alive(*e)) else {
            return report;
        };
        let scope = CommandScope::begin();
        self.call_trait(
            world,
            entity,
            GAME_INSTANCE_TRAIT_ID,
            "on_shutdown",
            &[],
            &mut report,
        );
        let mut objects: Vec<Entity> = self.game_instance_objects(world).into_iter().collect();
        objects.sort_by_key(|object| super::reconcile::depth(world, *object));
        for object in objects {
            self.stop(world, object, &mut report);
        }
        discard_commands(world, scope.finish());
        report
    }

    /// Remove the game instance's objects, after its session ended.
    pub fn despawn_game_instance(&mut self, world: &mut World) {
        let Some(entity) = self.game_instance.take() else {
            return;
        };
        if world.is_alive(entity) {
            // Its component instances and child objects go with it.
            world.despawn_tree(entity);
        }
    }
}
