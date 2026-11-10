//! Applying the world commands scripts queue (`world::spawn`,
//! `world::destroy`). Entry point: `ScriptDriver::apply_commands`.

use engine_backend::scene::SceneWorldExt;
use std::sync::Arc;

use engine_backend::scene::{ObjectType, SpawnObject, Transform};
use pulsar_class::ClassInstance;
use pulsar_scenedb::{Entity, World};

use super::{DriverReport, ScriptDriver};
use crate::scripting::commands::WorldCommand;
use pulsar_events::builtin::{EntityDestroyed, EntitySpawned};

/// Release what dropped commands reserved.
pub(super) fn discard_commands(world: &mut World, commands: Vec<WorldCommand>) {
    for command in commands {
        if let WorldCommand::Spawn { entity, .. } = command {
            if world.is_alive(entity) && world.stable_id_of(entity).is_none() {
                world.despawn(entity);
            }
        }
    }
}

impl ScriptDriver {
    pub(crate) fn apply_commands(
        &mut self,
        world: &mut World,
        commands: Vec<WorldCommand>,
        report: &mut DriverReport,
    ) {
        for command in commands {
            match command {
                WorldCommand::Spawn {
                    entity,
                    class,
                    parent,
                    position,
                } => self.apply_spawn(world, entity, &class, parent, position, report),
                WorldCommand::Destroy { entity } => self.apply_destroy(world, entity, report),
                WorldCommand::SetViewTarget { entity } => {
                    let target = (entity != Entity::DANGLING).then_some(entity);
                    self.set_view_target(world, target);
                }
            }
        }
    }

    pub(super) fn apply_spawn(
        &mut self,
        world: &mut World,
        entity: Entity,
        class: &str,
        parent: Option<Entity>,
        position: [f64; 3],
        report: &mut DriverReport,
    ) {
        if !world.is_alive(entity) {
            return;
        }
        let template = self.resolve_class_ref(class).and_then(|entry| {
            if let Some(template) = self.templates.get(&entry.id) {
                return Some(Arc::clone(template));
            }
            match entry.load_definition() {
                Ok(def) => {
                    let template = pulsar_class::template(&def);
                    self.templates
                        .insert(entry.id.clone(), Arc::clone(&template));
                    Some(template)
                }
                Err(error) => {
                    tracing::warn!(class = %entry.name, "Class definition unreadable: {error}");
                    None
                }
            }
        });
        let Some(template) = template else {
            let message = format!("world::spawn: class '{class}' is not in this project");
            tracing::warn!("{message}");
            report.failures.push(message);
            world.despawn(entity);
            return;
        };
        let parent = parent.filter(|p| world.is_alive(*p));
        // Parent-relative: a spawn under a parent is placed in that parent's space.
        let transform = Transform {
            position,
            ..Transform::default()
        };
        let def = &template.def;
        let stable_id = self.next_spawn_id(world, &def.name);
        let spec = SpawnObject {
            stable_id: Some(stable_id),
            name: def.name.clone(),
            parent,
            transform,
            visibility: Default::default(),
            object_type: ObjectType::Blueprint,
        };
        match pulsar_class::world::instantiate_template_into(
            world,
            &template,
            ClassInstance::default(),
            spec,
            entity,
        ) {
            Ok(_) => {
                self.publish(
                    pulsar_events::gamma::Channel::Global,
                    EntitySpawned {
                        entity: entity.bits(),
                    },
                );
                report.spawned.push(entity)
            }
            Err(error) => {
                let message = format!("world::spawn of '{}' failed: {error}", def.name);
                tracing::warn!("{message}");
                report.failures.push(message);
                world.despawn(entity);
            }
        }
    }

    /// A StableId for a spawned object: `<Class>_rt<n>`, `n` counting this
    /// driver's spawns, so a run spawns the same ids every time.
    pub(super) fn next_spawn_id(&mut self, world: &World, class_name: &str) -> String {
        loop {
            self.spawn_serial += 1;
            let id = format!("{class_name}_rt{}", self.spawn_serial);
            if world.entity_for(&id).is_none() {
                return id;
            }
        }
    }

    pub(super) fn apply_destroy(
        &mut self,
        world: &mut World,
        entity: Entity,
        report: &mut DriverReport,
    ) {
        if !world.is_alive(entity) {
            return;
        }
        // Parents before children; every script on the tree ends while its
        // components are still there.
        let mut tree = vec![entity];
        let mut next = 0;
        while next < tree.len() {
            let children = world.children_of(Some(tree[next]));
            tree.extend(children);
            next += 1;
        }
        for &node in &tree {
            self.stop(world, node, report);
        }
        for &node in &tree {
            self.publish(
                pulsar_events::gamma::Channel::Global,
                EntityDestroyed {
                    entity: node.bits(),
                },
            );
        }
        world.despawn_tree(entity);
        report.destroyed.push(entity);
    }
}
