//! Keeping script instances in step with the world's `ClassInstance`s:
//! journal reconcile, rescans, starting and stopping instances and globals.
//! Entry points: [`ScriptDriver::reconcile`], `end_play_all`.

use engine_backend::scene::SceneWorldExt;
use std::collections::{BTreeMap, HashMap, HashSet};

use pulsar_class::{ClassEntry, ClassInstance};
use pulsar_scenedb::{ChangeRead, Entity, World};
use pulsar_script_runtime::RuntimeError;
use pulsar_world_registry::pulsar_scene_model::attachments;
use serde_json::Value;

use super::config::{global_instance_id, instance_id_for};
use super::world_commands::discard_commands;
use super::{DriverReport, ScriptDriver, Tracked};
use crate::scripting::bind_class_slots;
use crate::scripting::commands::CommandScope;
use engine_fs::world_roles::EngineRole;
use pulsar_events::builtin::{EndPlay, EntityDestroyed};
use pulsar_events::entity_channel;

/// Scan order: hierarchy depth, then StableId (then entity bits, for
/// objects without one).
pub(super) fn sort_roots(roots: &mut [(usize, String, Entity)]) {
    roots.sort_by(|a, b| (a.0, &a.1, a.2.bits()).cmp(&(b.0, &b.1, b.2.bits())));
}

/// Hierarchy depth: 0 for a root object.
pub(super) fn depth(world: &World, entity: Entity) -> usize {
    let mut depth = 0;
    let mut cursor = world.parent_of(entity);
    while let Some(parent) = cursor {
        depth += 1;
        if depth > 4096 {
            break;
        }
        cursor = world.parent_of(parent);
    }
    depth
}
impl ScriptDriver {
    /// Bring the script instances in line with the world's
    /// `ClassInstance`s without running any event. World commands queued by
    /// `end_play` of stopped instances are applied.
    pub fn reconcile(&mut self, world: &mut World) -> DriverReport {
        let mut report = DriverReport::default();
        let mut scope = CommandScope::begin();
        self.reconcile_into(world, &mut report);
        let commands = scope.take();
        self.apply_commands(world, commands, &mut report);
        discard_commands(world, scope.finish());
        report
    }

    /// Run `end_play` on every running instance (shutdown). World changes
    /// scripts request now are dropped.
    ///
    /// With an event hub attached, every script subscription, queued
    /// handler call and timer is dropped and the hub's queue drained, so
    /// a stopped session leaves nothing behind on the hub.
    pub fn end_play_all(&mut self, world: &mut World) -> Vec<RuntimeError> {
        let scope = CommandScope::begin();
        // The game ends while the roles' scripts still live: the player lets
        // go of their pawn, then the game mode hears the game end.
        let mut report = DriverReport::default();
        self.end_possession(world, &mut report);
        if let Some(mode) = self.role_entity(EngineRole::GameMode) {
            let mode_trait = EngineRole::GameMode.spec().trait_id;
            self.call_trait_hook(world, mode, mode_trait, "on_game_end", &[], &mut report);
        }
        if let Some(events) = self.events.as_mut() {
            // No handler runs after end_play.
            events.clear();
        }
        crate::player_input::end_session(world);
        let mut errors = std::mem::take(&mut report.script_errors);
        errors.extend(self.runtime.end_play_all(world));
        discard_commands(world, scope.finish());
        // The roles' objects are the session's, not the level's: they go
        // with it, after their scripts ended.
        self.despawn_role_entities(world);
        if let Some(events) = self.events.as_mut() {
            events.clear();
        }
        errors
    }

    pub(super) fn reconcile_into(&mut self, world: &mut World, report: &mut DriverReport) {
        let reloads = std::mem::take(&mut *self.reloads.lock().unwrap_or_else(|p| p.into_inner()));
        if !reloads.is_empty() {
            // A class asset changed: spawn from its new definition.
            self.templates.clear();
        }
        for event in reloads {
            if let Some(class) = self.reload_class_for_asset_into(world, &event, report) {
                tracing::info!(class = %class, "Reloaded script class after an asset update");
            }
        }
        if !self.globals_started {
            self.globals_started = true;
            self.start_globals(world, report);
        }
        if !self.roles_started {
            self.roles_started = true;
            self.start_roles(world, report);
        }
        let dirty = match (self.cursor.as_mut(), self.owner_cursor.as_mut()) {
            (Some(cursor), Some(owner_cursor)) => {
                self.scratch.clear();
                let instances = world.read_changes(cursor, &mut self.scratch);
                let changed_instances = self.scratch.len();
                let owners = world.read_changes(owner_cursor, &mut self.scratch);
                match (instances, owners) {
                    (ChangeRead::Complete, ChangeRead::Complete) => {
                        let mut seen = HashSet::new();
                        Some(
                            self.scratch
                                .iter()
                                .enumerate()
                                // An owner change concerns a script only on a
                                // `ClassInstance` holder (enabled/disabled).
                                .filter(|(i, change)| {
                                    *i < changed_instances
                                        || world.get::<ClassInstance>(change.entity).is_some()
                                        || self.holders.contains_key(&change.entity)
                                })
                                .map(|(_, change)| self.object_of_holder(world, change.entity))
                                .filter(|entity| seen.insert(*entity))
                                .collect::<Vec<_>>(),
                        )
                    }
                    _ => {
                        tracing::info!(
                            "A ClassInstance or ComponentOwner journal overflowed; rescanning"
                        );
                        None
                    }
                }
            }
            _ => {
                // Opened before the scan under the same world borrow: every
                // later change is in the journal, nothing is missed.
                self.cursor = Some(world.open_change_cursor::<ClassInstance>());
                self.owner_cursor = Some(world.open_change_cursor::<attachments::ComponentOwner>());
                None
            }
        };
        match dirty {
            Some(entities) => {
                for entity in entities {
                    self.sync_entity(world, entity, report);
                }
            }
            None => self.rescan(world, report),
        }
    }

    /// The object a `ClassInstance` change on `holder` concerns: the
    /// holder's owner, or for a holder already gone, the object it was
    /// tracked for.
    pub(super) fn object_of_holder(&self, world: &World, holder: Entity) -> Entity {
        if world.is_alive(holder) {
            return attachments::object_of(world, holder);
        }
        self.holders.get(&holder).copied().unwrap_or(holder)
    }

    /// Stop instances whose object is gone and start or refresh every live
    /// `ClassInstance`, by depth then StableId.
    pub(super) fn rescan(&mut self, world: &mut World, report: &mut DriverReport) {
        let mut objects = HashSet::new();
        let mut roots: Vec<(usize, String, Entity)> = world
            .query::<&ClassInstance>()
            .map(|(holder, _)| attachments::object_of(world, holder))
            .filter(|object| objects.insert(*object))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|entity| {
                let stable = world.stable_id_of(entity).unwrap_or_default().to_owned();
                (depth(world, entity), stable, entity)
            })
            .collect();
        sort_roots(&mut roots);
        let live: HashSet<Entity> = roots.iter().map(|root| root.2).collect();
        let mut gone: Vec<Entity> = self
            .tracked
            .keys()
            .copied()
            .filter(|e| !live.contains(e))
            .collect();
        gone.sort_by_key(|entity| entity.bits());
        for entity in gone {
            self.stop(world, entity, report);
        }
        for (_, _, entity) in roots {
            self.sync_entity(world, entity, report);
        }
    }

    pub(super) fn sync_entity(
        &mut self,
        world: &mut World,
        entity: Entity,
        report: &mut DriverReport,
    ) {
        let holder = world
            .is_alive(entity)
            .then(|| pulsar_class::world::class_instance_entity(world, entity))
            .flatten();
        // A disabled `ClassInstance` runs no script, like an absent one.
        let Some(instance) = holder
            .filter(|holder| attachments::is_enabled(world, *holder))
            .and_then(|holder| world.get::<ClassInstance>(holder).cloned())
        else {
            self.stop(world, entity, report);
            return;
        };
        self.holders.retain(|_, object| *object != entity);
        self.holders.insert(holder.expect("resolved above"), entity);
        let Some(tracked) = self.tracked.get(&entity).cloned() else {
            self.start(world, entity, &instance, report);
            return;
        };
        let entry = self.resolve(&instance);
        let same_class = match &entry {
            Some(entry) => entry.id == tracked.class,
            None => instance.class == tracked.class && instance.class_name == tracked.class_name,
        };
        if !same_class {
            self.stop(world, entity, report);
            self.start(world, entity, &instance, report);
        } else if let Some(id) = &tracked.instance {
            // Same class, rewritten: its components may have been rebuilt.
            bind_class_slots(&mut self.runtime, id, world, entity);
        } else if let Some(entry) = entry {
            // Known object without a script yet: the module may exist now.
            self.try_spawn(world, entity, &instance.variable_overrides, &entry, report);
        }
    }

    pub(super) fn start(
        &mut self,
        world: &World,
        entity: Entity,
        instance: &ClassInstance,
        report: &mut DriverReport,
    ) {
        let Some(entry) = self.resolve(instance) else {
            let message = format!(
                "object '{}': class '{}' ({}) is not in this project; its script does not run",
                world.stable_id_of(entity).unwrap_or("?"),
                instance.class_name,
                instance.class
            );
            tracing::warn!("{message}");
            report.failures.push(message);
            self.tracked.insert(
                entity,
                Tracked {
                    class: instance.class.clone(),
                    class_name: instance.class_name.clone(),
                    instance: None,
                },
            );
            return;
        };
        self.tracked.insert(
            entity,
            Tracked {
                class: entry.id.clone(),
                class_name: entry.name.clone(),
                instance: None,
            },
        );
        self.try_spawn(world, entity, &instance.variable_overrides, &entry, report);
    }

    pub(super) fn try_spawn(
        &mut self,
        world: &World,
        entity: Entity,
        overrides: &BTreeMap<String, Value>,
        entry: &ClassEntry,
        report: &mut DriverReport,
    ) {
        let Some(class) = self.ensure_class_loaded(entry, report) else {
            return;
        };
        let stable = match world.stable_id_of(entity) {
            Some(stable) => stable.to_owned(),
            None => {
                tracing::warn!(class = %entry.name, "Class instance on an entity without a StableId; its instance id is not stable");
                format!("entity:{:x}", entity.bits())
            }
        };
        let id = instance_id_for(&stable, &entry.id);
        if self
            .by_instance
            .get(&id)
            .is_some_and(|other| *other != entity)
        {
            let message = format!("script instance '{id}' already runs on another entity");
            tracing::warn!("{message}");
            report.failures.push(message);
            return;
        }
        let overrides: HashMap<String, Value> = overrides.clone().into_iter().collect();
        let entity_for = |stable_id: &str| world.entity_for(stable_id);
        match self.runtime.spawn_with_json(
            id.clone(),
            &class,
            Some(entity),
            &overrides,
            &entity_for,
        ) {
            Ok(rejected) => {
                // A bad override keeps the class default; the instance runs.
                for error in rejected {
                    let message =
                        format!("script instance '{id}': {error}; using the class default");
                    tracing::warn!("{message}");
                    report.failures.push(message);
                }
                bind_class_slots(&mut self.runtime, &id, world, entity);
                if let Some(tracked) = self.tracked.get_mut(&entity) {
                    tracked.instance = Some(id.clone());
                }
                self.by_instance.insert(id.clone(), entity);
                self.subscribe_instance(
                    &id,
                    &class,
                    entry.id.as_str(),
                    Some(entity),
                    Some(world),
                    report,
                );
                report.started.push(id);
            }
            Err(error) => {
                let message = format!("script instance '{id}' did not start: {error}");
                tracing::warn!("{message}");
                report.failures.push(message);
            }
        }
    }

    pub(super) fn stop(&mut self, world: &mut World, entity: Entity, report: &mut DriverReport) {
        self.holders.retain(|_, object| *object != entity);
        let Some(tracked) = self.tracked.remove(&entity) else {
            return;
        };
        if let Some(id) = tracked.instance {
            self.by_instance.remove(&id);
            if let Some(events) = self.events.as_mut() {
                events.unsubscribe(&id);
            }
            let begun = self.runtime.has_begun(&id);
            if let Err(error) = self.runtime.despawn(&id, world) {
                tracing::warn!("{error}");
                report.script_errors.push(error);
            }
            if begun {
                self.publish(
                    entity_channel(entity.bits()),
                    EndPlay {
                        entity: entity.bits(),
                    },
                );
            }
            report.stopped.push(id);
        }
        if !world.is_alive(entity) {
            // Removed from the world by someone else (world::destroy
            // publishes its own before despawning).
            self.publish(
                pulsar_events::gamma::Channel::Global,
                EntityDestroyed {
                    entity: entity.bits(),
                },
            );
        }
    }

    pub(super) fn start_globals(&mut self, world: &World, report: &mut DriverReport) {
        for class_ref in self.config.global_scripts.clone() {
            let Some(entry) = self.resolve_class_ref(&class_ref) else {
                let message = format!("global script class '{class_ref}' is not in this project");
                tracing::warn!("{message}");
                report.failures.push(message);
                continue;
            };
            let Some(class) = self.ensure_class_loaded(&entry, report) else {
                let message = format!(
                    "global script class '{}' has no compiled script",
                    entry.name
                );
                tracing::warn!("{message}");
                report.failures.push(message);
                continue;
            };
            let id = global_instance_id(&entry.id);
            if self.globals.contains(&id) {
                continue;
            }
            match self.runtime.spawn(id.clone(), &class, None, &[]) {
                Ok(()) => {
                    self.globals.push(id.clone());
                    self.subscribe_instance(
                        &id,
                        &class,
                        entry.id.as_str(),
                        None,
                        Some(world),
                        report,
                    );
                    report.started.push(id);
                }
                Err(error) => {
                    let message = format!("global script '{id}' did not start: {error}");
                    tracing::warn!("{message}");
                    report.failures.push(message);
                }
            }
        }
    }
}
