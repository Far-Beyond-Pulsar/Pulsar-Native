//! Roles (every registered role: the engine's game mode, game state, player
//! controller and character, and any plugin's), local player possession and
//! trait hooks. Entry points: `start_roles`, `run_role_hooks`,
//! `advance_possession`.

use engine_backend::scene::SceneWorldExt;
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::pulsar_scene_model::attachments;

use super::{DriverReport, ScriptDriver};
use crate::roles::RoleEntity;
use crate::scripting::commands::WorldCommand;
use crate::scripting::traits::{DispatchTarget, HookOutcome, ScriptTarget, TraitCheck};

// ---- roles ----------------------------------------------------------------------
//
// A level assigns implementers to the registered roles
// (`pulsar_traits::roles`): the engine's game mode, game state, player
// controller and player character, and any plugin's. Each role is a trait
// (`GameMode`, ...) whose methods its implementer provides.
//
// At the session's first script phase every usable role is spawned, in the
// registry's order, as an
// **entity of its class** through the world-command path `world::spawn` uses
// (`apply_spawn`), so a role has what any bound object has: an entity, a
// `ClassInstance`, `Self`-scoped subscriptions on its own channel. The entities
// are the session's, not the level's: the driver lists the ones it owns and
// despawns them in `end_play_all`, and a `RoleEntity` stamp lets a later
// session over the same world re-bind a leftover instead of duplicating it.
//
// A role that starts `Now` (the game mode, game state and controller) is
// spawned and its script started within the phase; one that starts at the
// `EndOfFrame` (the character) is queued like a script's spawn, so it runs a
// frame later and possession follows then. Every spawn is announced on the
// hub (`RoleSpawned`), and `GameBegan` follows the engine's start hooks, so a
// plugin's role reacts to play wherever the engine's do. The engine's four
// get their session behaviour here (hooks, possession); the trait methods are
// called through the one dispatcher, see [`super::super::traits`].

use pulsar_traits::builtin::{
    GAME_MODE_ROLE, GAME_MODE_TRAIT_ID, GAME_STATE_ROLE, GAME_STATE_TRAIT_ID,
    PLAYER_CHARACTER_ROLE, PLAYER_CHARACTER_TRAIT_ID, PLAYER_CONTROLLER_ROLE,
    PLAYER_CONTROLLER_TRAIT_ID,
};
use pulsar_traits::roles::RoleStart;

/// A trait method to call on a role entity once its script has begun.
pub(super) struct RoleHook {
    entity: Entity,
    trait_id: &'static str,
    method: &'static str,
    args: Vec<pulsar_script_vm::Value>,
}

/// An entity this session spawned for a role.
pub(super) struct SessionRole {
    role: &'static str,
    entity: Entity,
}

/// The local player's character between being queued for spawn and running.
pub(super) struct Possession {
    character: Entity,
    /// The player's controller entity, when the level has one.
    controller: Option<Entity>,
}

/// The local player's controller in control of a pawn.
pub(super) struct Possessed {
    controller: Option<Entity>,
    pawn: Entity,
    /// The component instances the plugins' session hooks attached to the
    /// pawn for the possession (the input plugin's mapping).
    attached: Vec<Entity>,
}

/// Stamps a component instance a possession attached to its pawn, so the next
/// session over the same world can tell it from one the pawn's class placed
/// itself.
#[derive(Clone, Copy, Debug)]
pub(super) struct PossessionAttachment;

/// The id of the one local player the session starts.
pub(super) const LOCAL_PLAYER: i64 = crate::view_target::LOCAL_PLAYER;

impl ScriptDriver {
    /// The implementers the level assigned to the roles. Takes effect when
    /// the session's first script phase runs.
    pub fn set_world_roles(&mut self, roles: pulsar_traits::WorldRoles) {
        self.world_roles = roles;
    }

    /// Share the flag this driver sets while a pawn is possessed (the tick
    /// loop hands it out as `TickLoop::possession_gate`).
    pub fn set_possession_gate(&mut self, gate: crate::possession::PossessionGate) {
        gate.set(self.possessed.is_some());
        self.possession_gate = gate;
    }

    /// Share the view targets this driver writes (the tick loop hands them
    /// out as `TickLoop::view_targets`). Targets already set move over.
    pub fn set_view_targets(&mut self, targets: crate::view_target::ViewTargets) {
        if let Some(target) = self.view_targets.local() {
            targets.set(LOCAL_PLAYER, target);
        }
        self.view_targets = targets;
    }

    /// Make the local player's view follow `target`: the best enabled
    /// `CameraComponent` on it, else beneath it, is the frame's camera until
    /// the target changes. `None` clears it (the world's own camera selection
    /// shows the game). A target that is not alive is refused. Possession sets
    /// this to the pawn and clears it when the possession ends; this is the
    /// seam for gameplay that looks elsewhere in between.
    pub fn set_view_target(&mut self, world: &World, target: Option<Entity>) -> bool {
        match target {
            Some(target) if !world.is_alive(target) => {
                tracing::warn!("The view target is not a live entity; the view is unchanged");
                false
            }
            Some(target) => {
                self.view_targets.set(LOCAL_PLAYER, target);
                true
            }
            None => {
                self.view_targets.clear(LOCAL_PLAYER);
                true
            }
        }
    }

    /// The pawn the local player's controller possesses, once possession
    /// completed and until it ends.
    pub fn possessed_pawn(&self) -> Option<Entity> {
        self.possessed.as_ref().map(|possessed| possessed.pawn)
    }

    /// The entities this session spawned for the roles, by role id.
    pub fn role_entities(&self) -> Vec<(&'static str, Entity)> {
        self.role_entities
            .iter()
            .map(|owned| (owned.role, owned.entity))
            .collect()
    }

    /// The entity of the role `role` (its id), if the session spawned one.
    pub fn role_entity(&self, role: &str) -> Option<Entity> {
        self.role_entities
            .iter()
            .find(|owned| owned.role == role)
            .map(|owned| owned.entity)
    }

    /// Does the class `class_ref` (a GUID or a name) implement the trait
    /// `trait_ref` names (an id, or anything `TraitDb::resolve` accepts)?
    /// `false` also when either is unknown or the class's declarations
    /// cannot be read; see [`TraitCheck`].
    pub fn class_implements(&mut self, class_ref: &str, trait_ref: &str) -> bool {
        let Some(entry) = self.resolve_class_ref(class_ref) else {
            return false;
        };
        self.traits.check(&self.project_root, &entry, trait_ref) == TraitCheck::Implements
    }

    /// Does the class of `entity` implement the trait `trait_ref` names?
    /// The class is the entity's `ClassInstance`, resolved through the class
    /// registry; the trait is resolved by id, supertraits included.
    pub fn entity_implements(&mut self, world: &World, entity: Entity, trait_ref: &str) -> bool {
        self.entity_trait_check(world, entity, trait_ref) == TraitCheck::Implements
    }

    /// [`entity_implements`](Self::entity_implements) keeping "cannot tell"
    /// apart from "no".
    pub fn entity_trait_check(
        &mut self,
        world: &World,
        entity: Entity,
        trait_ref: &str,
    ) -> TraitCheck {
        let Some(instance) = pulsar_class::world::class_instance_of(world, entity) else {
            return TraitCheck::DoesNot;
        };
        let Some(entry) = self.resolve(&instance) else {
            return TraitCheck::DoesNot;
        };
        self.traits.check(&self.project_root, &entry, trait_ref)
    }

    /// Spawn the role entities, in the registry's order: the game mode, game
    /// state and the local player's controller now, their character at the
    /// end of the frame, then any plugin's. An unset role is skipped with a
    /// log line. Each spawn is announced on the hub, then the engine's start
    /// hooks are queued and `GameBegan` follows them.
    pub(super) fn start_roles(&mut self, world: &mut World, report: &mut DriverReport) {
        let roles = self.world_roles.clone();
        // Even with no roles: an entity a previous session left must not linger.
        self.adopt_leftover_roles(world, &roles);
        if roles.is_empty() {
            tracing::info!("No roles are set for this level; nothing was instantiated");
        }
        for message in roles.diagnostics() {
            tracing::warn!("{message}");
        }

        let mut spawned: Vec<(&'static str, Entity)> = Vec::new();
        for role in pulsar_traits::roles::roles() {
            let now = role.start == RoleStart::Now;
            if let Some(entity) = self.spawn_role(&roles, role.id, world, now, report) {
                spawned.push((role.id, entity));
                self.publish(
                    pulsar_events::gamma::Channel::Global,
                    pulsar_events::builtin::RoleSpawned {
                        role: role.id.to_owned(),
                        entity: entity.bits(),
                    },
                );
            }
        }
        let entity_of = |id: &str| {
            spawned
                .iter()
                .find(|(role, _)| *role == id)
                .map(|(_, entity)| *entity)
        };
        let mode = entity_of(GAME_MODE_ROLE);
        let state = entity_of(GAME_STATE_ROLE);
        let controller = entity_of(PLAYER_CONTROLLER_ROLE);
        let character = entity_of(PLAYER_CHARACTER_ROLE);
        if let Some(entity) = mode {
            self.queue_hook(entity, GAME_MODE_TRAIT_ID, "on_game_start", vec![]);
        }
        if let Some(entity) = state {
            self.queue_hook(entity, GAME_STATE_TRAIT_ID, "on_init", vec![]);
        }
        // The local player joins: the game mode hears of them, then the game
        // state; their controller is already up and their character spawns.
        let player = pulsar_script_vm::Value::Int(LOCAL_PLAYER);
        if let Some(entity) = mode {
            self.queue_hook(
                entity,
                GAME_MODE_TRAIT_ID,
                "on_player_joined",
                vec![player.clone()],
            );
        }
        if let Some(entity) = state {
            self.queue_hook(entity, GAME_STATE_TRAIT_ID, "on_player_added", vec![player]);
        }
        // Queued like a script's `world::spawn`, applied at the end of this
        // frame: the character runs next frame and possession follows.
        if let Some(character) = character {
            self.pending_possession = Some(Possession {
                character,
                controller,
            });
        }
        self.game_began_pending = true;
    }

    /// Spawn the entity of `role` from its class. With `now` the spawn
    /// command is applied here and the script started, so the role runs this
    /// frame; otherwise it is queued for the end of the frame. A role entity
    /// that a previous session left behind and `adopt_leftover_roles` kept
    /// is used instead of a new one.
    pub(super) fn spawn_role(
        &mut self,
        roles: &pulsar_traits::WorldRoles,
        role: &'static str,
        world: &mut World,
        now: bool,
        report: &mut DriverReport,
    ) -> Option<Entity> {
        let label = pulsar_traits::roles::role(role).map_or(role, |r| r.label);
        let Some(binding) = roles.usable(role) else {
            if roles.get(role).is_none() {
                tracing::info!("{label} is not set for this level; skipped");
            }
            return None;
        };
        let Some(class_ref) = binding.class_name() else {
            tracing::warn!("{label} '{}' has no class name", binding.asset);
            return None;
        };
        let Some(entry) = self.resolve_class_ref(class_ref) else {
            let message = format!("{label} class '{class_ref}' is not in this project");
            tracing::warn!("{message}");
            report.failures.push(message);
            return None;
        };

        let entity = match self.role_entity(role) {
            Some(existing) => {
                tracing::info!(class = %entry.name, "{label} re-bound to its existing entity");
                existing
            }
            None => {
                let entity = world.spawn();
                let command = WorldCommand::Spawn {
                    entity,
                    class: entry.id.as_str().to_owned(),
                    parent: None,
                    position: [0.0; 3],
                };
                self.role_entities.push(SessionRole { role, entity });
                if now {
                    self.apply_commands(world, vec![command], report);
                    if !world.is_alive(entity) {
                        // The spawn failed and said why.
                        self.role_entities.retain(|owned| owned.entity != entity);
                        return None;
                    }
                } else {
                    crate::scripting::commands::queue(command);
                }
                entity
            }
        };
        if now {
            // Started here rather than by the scan that follows, so the
            // roles begin in role order, before the level's own objects.
            self.sync_entity(world, entity, report);
            self.mark_role_entities(world);
            if self
                .tracked
                .get(&entity)
                .is_some_and(|tracked| tracked.instance.is_none())
            {
                let message = format!("{label} class '{}' has no compiled script", entry.name);
                tracing::warn!("{message}");
                report.failures.push(message);
            }
        }
        tracing::info!(class = %entry.name, "{label} spawned");
        Some(entity)
    }

    /// Role entities a previous session left in a world this one adopted
    /// (a hot reload that did not stop it cleanly). One the level still
    /// assigns, of the same class, is kept and re-bound; any other is
    /// removed. Role entities are never level data, so nothing else could
    /// have put them there.
    pub(super) fn adopt_leftover_roles(
        &mut self,
        world: &mut World,
        roles: &pulsar_traits::WorldRoles,
    ) {
        let leftovers: Vec<(Entity, &'static str)> = world
            .query::<&RoleEntity>()
            .map(|(entity, marker)| (entity, marker.role))
            .collect();
        for (entity, role) in leftovers {
            if self
                .role_entities
                .iter()
                .any(|owned| owned.entity == entity)
            {
                continue;
            }
            let wanted = roles
                .usable(role)
                .and_then(|binding| binding.class_name().map(str::to_owned))
                .and_then(|class_ref| self.resolve_class_ref(&class_ref));
            let present = pulsar_class::world::class_instance_of(world, entity)
                .and_then(|instance| self.resolve(&instance));
            let same_class = matches!((&wanted, &present), (Some(w), Some(p)) if w.id == p.id);
            if same_class && self.role_entity(role).is_none() {
                tracing::info!(?role, "Re-binding a role entity left by a previous session");
                self.role_entities.push(SessionRole { role, entity });
            } else {
                tracing::info!(
                    ?role,
                    "Removing a stale role entity left by a previous session"
                );
                world.despawn_tree(entity);
            }
        }
    }

    /// Stamp the session's role entities that exist now (the character's
    /// spawn is applied at the end of the frame it was queued in).
    pub(super) fn mark_role_entities(&mut self, world: &mut World) {
        for owned in &self.role_entities {
            if self.tracked.contains_key(&owned.entity)
                && world.is_alive(owned.entity)
                && world.get::<RoleEntity>(owned.entity).is_none()
            {
                world.insert(owned.entity, RoleEntity { role: owned.role });
            }
        }
    }

    /// Despawn the entities this session spawned for the roles. They are the
    /// session's: nothing of them may outlive it in a world it borrowed.
    pub(super) fn despawn_role_entities(&mut self, world: &mut World) {
        self.role_hooks.clear();
        self.pending_possession = None;
        self.possessed = None;
        self.possession_gate.set(false);
        self.view_targets.clear_all();
        for owned in std::mem::take(&mut self.role_entities) {
            let entity = owned.entity;
            self.holders.retain(|_, object| *object != entity);
            if let Some(tracked) = self.tracked.remove(&entity) {
                if let Some(id) = tracked.instance {
                    self.by_instance.remove(&id);
                }
            }
            if world.is_alive(entity) {
                world.despawn_tree(entity);
            }
        }
    }

    pub(super) fn queue_hook(
        &mut self,
        entity: Entity,
        trait_id: &'static str,
        method: &'static str,
        args: Vec<pulsar_script_vm::Value>,
    ) {
        self.role_hooks.push(RoleHook {
            entity,
            trait_id,
            method,
            args,
        });
    }

    /// Report a contract problem once per session.
    pub(super) fn note_once(&mut self, report: &mut DriverReport, message: String) {
        if self.noted.insert(message.clone()) {
            tracing::warn!("{message}");
            report.failures.push(message);
        }
    }

    /// Call the method `method` of the trait `trait_ref` names on `entity`,
    /// through the one dispatcher (`RuntimeTraits::dispatch`): the entity's
    /// script class's handler and its implementing components' bound methods.
    /// Problems are reported once per session.
    pub(crate) fn call_trait(
        &mut self,
        world: &mut World,
        entity: Entity,
        trait_ref: &str,
        method: &str,
        args: &[pulsar_script_vm::Value],
        report: &mut DriverReport,
    ) -> HookOutcome {
        let script = self.tracked.get(&entity).cloned().and_then(|tracked| {
            let instance = tracked.instance?;
            let runtime_class = self.runtime.class_of(&instance)?.to_owned();
            let entry = self.registry.by_id(&tracked.class).cloned()?;
            Some(ScriptTarget {
                entry,
                instance,
                runtime_class,
            })
        });
        let target = DispatchTarget { entity, script };
        let mut problems = Vec::new();
        let outcome = self.traits.dispatch(
            &self.project_root,
            &mut self.runtime,
            world,
            &target,
            trait_ref,
            method,
            args,
            report,
            &mut problems,
        );
        for message in problems {
            self.note_once(report, message);
        }
        outcome
    }

    /// Run the hooks of role entities that began this frame, then finish
    /// the local player's possession once their character is running. While
    /// a pawn is possessed the controller hears `on_input` every frame (from
    /// the frame after possession began).
    pub(super) fn run_role_hooks(&mut self, world: &mut World, report: &mut DriverReport) {
        self.mark_role_entities(world);
        for hook in std::mem::take(&mut self.role_hooks) {
            self.call_trait(
                world,
                hook.entity,
                hook.trait_id,
                hook.method,
                &hook.args,
                report,
            );
        }
        if std::mem::take(&mut self.game_began_pending) {
            self.publish(
                pulsar_events::gamma::Channel::Global,
                pulsar_events::builtin::GameBegan {
                    level: self.level.clone(),
                },
            );
        }
        self.drop_lost_pawn(world, report);
        self.controller_input(world, report);
        self.advance_possession(world, report);
    }

    /// `PlayerController.on_input(delta_seconds)`, while a pawn is possessed.
    pub(super) fn controller_input(&mut self, world: &mut World, report: &mut DriverReport) {
        let Some(controller) = self.possessed.as_ref().and_then(|p| p.controller) else {
            return;
        };
        let delta = pulsar_script_vm::Value::Float(self.frame_delta);
        self.call_trait(
            world,
            controller,
            PLAYER_CONTROLLER_TRAIT_ID,
            "on_input",
            &[delta],
            report,
        );
    }

    /// The possessed pawn was destroyed (a script's `world::destroy`, the
    /// editor deleting it): the possession ends with it.
    pub(super) fn drop_lost_pawn(&mut self, world: &mut World, report: &mut DriverReport) {
        if self
            .possessed
            .as_ref()
            .is_some_and(|possessed| !world.is_alive(possessed.pawn))
        {
            self.end_possession(world, report);
        }
    }

    /// End the local player's possession: the controller hears
    /// `on_unpossess`, the pawn (when it still exists) `on_unpossessed`, what
    /// the plugins attached for the possession leaves the pawn and the free
    /// camera may fly again.
    /// Nothing is possessed afterwards. The seam for a later "possess another
    /// pawn".
    pub(crate) fn end_possession(&mut self, world: &mut World, report: &mut DriverReport) {
        let Some(possessed) = self.possessed.take() else {
            return;
        };
        if let Some(controller) = possessed.controller {
            self.call_trait(
                world,
                controller,
                PLAYER_CONTROLLER_TRAIT_ID,
                "on_unpossess",
                &[],
                report,
            );
        }
        if world.is_alive(possessed.pawn) {
            self.call_trait(
                world,
                possessed.pawn,
                PLAYER_CHARACTER_TRAIT_ID,
                "on_unpossessed",
                &[],
                report,
            );
        }
        for instance in possessed.attached {
            attachments::detach(world, instance);
        }
        self.possession_gate.set(false);
        self.view_targets.clear(LOCAL_PLAYER);
        self.publish(
            pulsar_events::gamma::Channel::Global,
            pulsar_events::builtin::PossessionChanged {
                controller: possessed.controller.map_or(0, Entity::bits),
                pawn: possessed.pawn.bits(),
                possessed: false,
            },
        );
        tracing::info!("The player controller no longer possesses its pawn");
    }

    /// Run every plugin's possession hook on `pawn`
    /// (`engine_backend::scene::game_hooks::GameSessionRegistration`): the
    /// input plugin puts the level's mapping to work on it. Returns what they
    /// attached; a hook that fails is reported once and the possession
    /// carries on without it.
    pub(super) fn possession_hooks(
        &mut self,
        world: &mut World,
        pawn: Entity,
        report: &mut DriverReport,
    ) -> Vec<Entity> {
        // Left by a previous session over this world that never shut down.
        for stale in attachments::instances(world, pawn) {
            if world.get::<PossessionAttachment>(stale).is_some() {
                attachments::detach(world, stale);
            }
        }
        let context = engine_backend::scene::game_hooks::PossessionContext {
            project_root: &self.project_root,
            roles: &self.world_roles,
        };
        let mut attached = Vec::new();
        let mut failures = Vec::new();
        for hook in engine_backend::scene::game_hooks::game_sessions() {
            let Some(possess) = hook.possess else {
                continue;
            };
            match possess(world, pawn, &context) {
                Ok(Some(instance)) => {
                    world.insert(instance, PossessionAttachment);
                    attached.push(instance);
                }
                Ok(None) => {}
                Err(message) => failures.push(message),
            }
        }
        for message in failures {
            self.note_once(report, message);
        }
        attached
    }

    /// The character is spawned by a world command at the end of the frame
    /// the roles started, so it runs one frame later. Then the pawn is
    /// checked against the `PlayerCharacter` trait, the character hears it
    /// was spawned, the controller possesses it, the character learns of
    /// its controller and the plugins' possession hooks run on the pawn
    /// (`possession_hooks`: the input plugin attaches the level's mapping,
    /// so the character's Blueprint hears `input::<Action>` on its own
    /// channel). Last, the player's view target
    /// becomes the pawn (`follow_pawn_view`).
    pub(super) fn advance_possession(&mut self, world: &mut World, report: &mut DriverReport) {
        let Some(character) = self.pending_possession.as_ref().map(|p| p.character) else {
            return;
        };
        let running = self
            .tracked
            .get(&character)
            .is_some_and(|tracked| tracked.instance.is_some());
        if !running {
            // The reserved entity is alive until its spawn command ran; if it
            // is gone the spawn failed (already logged).
            if !world.is_alive(character) {
                tracing::warn!("The player character was not spawned; possession skipped");
                self.pending_possession = None;
            }
            return;
        }
        let Some(pending) = self.pending_possession.take() else {
            return;
        };
        let character_trait = PLAYER_CHARACTER_TRAIT_ID;
        if self.entity_trait_check(world, character, character_trait) == TraitCheck::DoesNot {
            let message = format!(
                "The possessed entity does not implement {character_trait}; possession skipped"
            );
            self.note_once(report, message);
            return;
        }
        self.call_trait(world, character, character_trait, "on_spawned", &[], report);
        if let Some(controller) = pending.controller {
            let pawn = pulsar_script_vm::Value::Int(character.bits() as i64);
            self.call_trait(
                world,
                controller,
                PLAYER_CONTROLLER_TRAIT_ID,
                "on_possess",
                &[pawn],
                report,
            );
        }
        let controller_id = pulsar_script_vm::Value::Int(
            pending
                .controller
                .map_or(0, |controller| controller.bits() as i64),
        );
        self.call_trait(
            world,
            character,
            character_trait,
            "on_possessed",
            &[controller_id],
            report,
        );
        let attached = self.possession_hooks(world, character, report);
        self.possessed = Some(Possessed {
            controller: pending.controller,
            pawn: character,
            attached,
        });
        self.possession_gate.set(true);
        self.publish(
            pulsar_events::gamma::Channel::Global,
            pulsar_events::builtin::PossessionChanged {
                controller: pending.controller.map_or(0, Entity::bits),
                pawn: character.bits(),
                possessed: true,
            },
        );
        self.follow_pawn_view(world, character);
    }

    /// The local player's view follows the pawn they possess: its camera
    /// (on the pawn, else beneath it) becomes the frame's camera. A pawn with
    /// no camera still becomes the target, so a camera attached to it later
    /// is picked up; until then the level's other cameras or the free camera
    /// show the game (the free camera only stands down for a pawn camera),
    /// which is said once.
    pub(super) fn follow_pawn_view(&mut self, world: &World, pawn: Entity) {
        self.view_targets.set(LOCAL_PLAYER, pawn);
        if helio_component::components::camera_on_object(world, pawn).is_none() {
            let message = "The possessed pawn has no enabled CameraComponent on it or beneath it: \
                           the view stays on the level's camera or the free camera (add a \
                           CameraComponent to the character's Blueprint)"
                .to_owned();
            if self.noted.insert(message.clone()) {
                tracing::warn!("{message}");
            }
        }
    }
}
