//! Keeping script instances in step with the world's `ClassInstance`s: the
//! component lifecycle's `begin_play`, `construct` and `end_play` start,
//! rebind and stop them; globals start here too.
//! Entry points: [`ScriptDriver::reconcile`], `end_play_all`.

use engine_backend::scene::SceneWorldExt;
use std::collections::{BTreeMap, HashMap};

use pulsar_class::{ClassEntry, ClassEvent, ClassInbox, ClassInstance};
use pulsar_scenedb::{Entity, World};
use pulsar_script_runtime::RuntimeError;
use pulsar_world_registry::pulsar_scene_model::attachments;
use serde_json::Value;

use super::config::{global_instance_id, instance_id_for};
use super::world_commands::discard_commands;
use super::{DriverReport, ScriptDriver, Tracked};
use crate::scripting::bind_class_slots;
use crate::scripting::commands::CommandScope;
use pulsar_events::builtin::{EndPlay, EntityDestroyed};
use pulsar_events::entity_channel;

/// Start order: hierarchy depth, then StableId (then entity bits, for
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
/// The scene identity of a driver's own lifecycle (it has one world).
const OWN_SCENE: usize = 1;

/// The lifecycle a driver pumps when no host session does: a session of its
/// own and a private event hub.
pub(super) struct OwnLifecycle {
    session: pulsar_world_registry::Session,
    hub: pulsar_events::EventHub,
}

impl OwnLifecycle {
    pub(super) fn new() -> Self {
        Self {
            session: pulsar_world_registry::Session::new(
                pulsar_world_registry::SessionKind::Headless,
            ),
            hub: pulsar_events::EventHub::new(),
        }
    }
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
        profiling::profile_scope!("Script::end_play");
        let scope = CommandScope::begin();
        // A driver pumping its own lifecycle ends it here, with the world
        // alive. Either way the ends the lifecycle posted are dropped: the
        // runtime ends every instance below, in start order, and nothing
        // starts at shutdown.
        if let Some(own) = self.own_lifecycle.as_mut() {
            pulsar_world_registry::end_session(world, &own.hub, &mut own.session);
        }
        self.inbox.take();
        // The game ends while the roles' scripts still live: the player lets
        // go of their pawn, then the game mode hears the game end.
        let mut report = DriverReport::default();
        self.end_possession(world, &mut report);
        if let Some(mode) = self.role_entity(pulsar_traits::builtin::GAME_MODE_ROLE) {
            self.call_trait(
                world,
                mode,
                pulsar_traits::builtin::GAME_MODE_TRAIT_ID,
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
        if let Some(events) = self.events.as_mut() {
            // No handler runs after end_play.
            events.clear();
        }
        for hook in engine_backend::scene::game_hooks::game_sessions() {
            if let Some(end) = hook.end {
                end(world);
            }
        }
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
        self.pump_own_lifecycle(world);
        self.apply_lifecycle(world, report);
    }

    /// Without a host session, deliver the component lifecycle ourselves.
    fn pump_own_lifecycle(&mut self, world: &mut World) {
        let Some(own) = self.own_lifecycle.as_mut() else {
            return;
        };
        if own.session.scope().resource::<ClassInbox>().is_none() {
            own.session.scope_mut().insert_resource(self.inbox.clone());
        }
        pulsar_world_registry::tick_live_components(
            world,
            &own.hub,
            self.frame_delta as f32,
            OWN_SCENE,
            &mut own.session,
        );
    }

    /// Run what the component lifecycle delivered since the last frame:
    /// ends first, then constructs, then the instances that began, by
    /// hierarchy depth, then StableId.
    fn apply_lifecycle(&mut self, world: &mut World, report: &mut DriverReport) {
        profiling::profile_scope!("Script::apply_lifecycle");
        let mut ends = Vec::new();
        let mut constructs = Vec::new();
        let mut begins: HashMap<Entity, Entity> = HashMap::new();
        for event in self.inbox.take() {
            match event {
                ClassEvent::End { object, .. } => ends.push(object),
                ClassEvent::Construct { object, instance } => constructs.push((object, instance)),
                ClassEvent::Begin { object, instance } => {
                    begins.insert(object, instance);
                }
            }
        }
        for object in ends {
            self.stop(world, object, report);
        }
        for (object, instance) in constructs {
            self.construct_object(world, object, instance, report);
        }
        let mut roots: Vec<(usize, String, Entity)> = begins
            .keys()
            .map(|&entity| {
                let stable = world.stable_id_of(entity).unwrap_or_default().to_owned();
                (depth(world, entity), stable, entity)
            })
            .collect();
        sort_roots(&mut roots);
        for (_, _, object) in roots {
            self.begin_object(world, object, begins[&object], report);
        }
    }

    /// The enabled `ClassInstance` on component instance `holder`. A disabled
    /// one runs no script, like an absent one.
    fn live_instance(world: &World, holder: Entity) -> Option<ClassInstance> {
        (world.is_alive(holder) && attachments::is_enabled(world, holder))
            .then(|| world.get::<ClassInstance>(holder).cloned())
            .flatten()
    }

    /// Start the script of `entity` now, ahead of the lifecycle's own
    /// `begin_play` (the roles begin in role order, before the level's own
    /// objects); that `begin_play` then finds it running.
    pub(super) fn start_now(
        &mut self,
        world: &mut World,
        entity: Entity,
        report: &mut DriverReport,
    ) {
        if let Some(holder) = world
            .is_alive(entity)
            .then(|| pulsar_class::world::class_instance_entity(world, entity))
            .flatten()
        {
            self.begin_object(world, entity, holder, report);
        }
    }

    /// `begin_play` of a `ClassInstance`: start its script, unless the roles
    /// already did.
    fn begin_object(
        &mut self,
        world: &mut World,
        object: Entity,
        holder: Entity,
        report: &mut DriverReport,
    ) {
        if !world.is_alive(object) {
            return;
        }
        let Some(instance) = Self::live_instance(world, holder) else {
            return;
        };
        if self.tracked.contains_key(&object) {
            self.sync_tracked(world, object, &instance, report);
        } else {
            self.start(world, object, &instance, report);
        }
    }

    /// `construct` of a `ClassInstance`: restart a script whose class
    /// changed, bind the slots again for the same class. An object that has
    /// not begun is left to its `begin_play`.
    fn construct_object(
        &mut self,
        world: &mut World,
        object: Entity,
        holder: Entity,
        report: &mut DriverReport,
    ) {
        if !self.tracked.contains_key(&object) {
            return;
        }
        if let Some(instance) = Self::live_instance(world, holder) {
            self.sync_tracked(world, object, &instance, report);
        }
    }

    fn sync_tracked(
        &mut self,
        world: &mut World,
        entity: Entity,
        instance: &ClassInstance,
        report: &mut DriverReport,
    ) {
        let Some(tracked) = self.tracked.get(&entity).cloned() else {
            return;
        };
        let entry = self.resolve(instance);
        let same_class = match &entry {
            Some(entry) => entry.id == tracked.class,
            None => instance.class == tracked.class && instance.class_name == tracked.class_name,
        };
        if !same_class {
            self.stop(world, entity, report);
            self.start(world, entity, instance, report);
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
