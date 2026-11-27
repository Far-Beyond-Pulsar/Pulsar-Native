//! The scene's table of contents: every entity, archetype and component type,
//! without any component's value.
//!
//! The scan takes the scene's lock in slices ([`CHUNK`] entities at a time),
//! so the renderer, which writes the scene every frame, never waits for a
//! whole scan. A scene changing between slices can leave one scan slightly
//! behind; the next one is right.

use std::collections::HashMap;
use std::sync::Arc;

use engine_backend::scene::SharedScene;
use pulsar_scene_model::{ComponentMeta, ComponentOwner, Name, Parent, SiblingIndex, StableId};
use pulsar_scenedb::{ComponentId, Entity, World};

use super::format;

/// Entities read per lock acquisition.
pub const CHUNK: usize = 2048;

#[derive(Clone, Debug)]
pub struct TypeRow {
    pub id: ComponentId,
    /// Without module paths.
    pub name: Arc<str>,
    pub full: &'static str,
    /// The registered component class this type is, if it is one.
    pub class: Option<&'static str>,
    /// Bytes per value in its column.
    pub size: usize,
    /// Entities that have it.
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct ArchetypeRow {
    pub id: u32,
    /// Indices into [`SceneIndex::types`], by name.
    pub types: Vec<u32>,
    /// Indices into [`SceneIndex::entities`], in row order.
    pub entities: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    /// A scene object (it has a [`StableId`]).
    Object,
    /// A component instance attached to an object.
    Instance {
        owner: Entity,
        enabled: bool,
    },
    Other,
}

#[derive(Clone, Debug)]
pub struct EntityRow {
    pub entity: Entity,
    /// Index into [`SceneIndex::archetypes`].
    pub archetype: u32,
    /// The object's name, the instance's class, or what stands in for one.
    pub label: Box<str>,
    pub kind: EntityKind,
    pub parent: Option<Entity>,
    pub order: u64,
    /// Lowercased text a plain search term is matched against.
    pub haystack: Box<str>,
}

#[derive(Debug, Default)]
pub struct SceneIndex {
    /// [`World::revision`] when the scan started.
    pub revision: u64,
    pub entities: Vec<EntityRow>,
    pub archetypes: Vec<ArchetypeRow>,
    /// By name.
    pub types: Vec<TypeRow>,
    pub by_entity: HashMap<Entity, u32>,
    pub object_count: usize,
    /// Root objects, in sibling order.
    pub roots: Vec<u32>,
    /// Child objects by parent's index, in sibling order.
    pub children: HashMap<u32, Vec<u32>>,
}

impl SceneIndex {
    pub fn entity(&self, entity: Entity) -> Option<&EntityRow> {
        self.by_entity
            .get(&entity)
            .and_then(|&index| self.entities.get(index as usize))
    }

    pub fn type_row(&self, id: ComponentId) -> Option<&TypeRow> {
        self.types.iter().find(|row| row.id == id)
    }

    /// Every entity that has component `id`, archetype by archetype in row
    /// order.
    pub fn entities_with(&self, id: ComponentId) -> Vec<u32> {
        let Some(type_ix) = self.types.iter().position(|row| row.id == id) else {
            return Vec::new();
        };
        self.archetypes
            .iter()
            .filter(|archetype| archetype.types.contains(&(type_ix as u32)))
            .flat_map(|archetype| archetype.entities.iter().copied())
            .collect()
    }
}

/// The registered component classes by the component id they are stored as.
pub fn class_names() -> HashMap<ComponentId, &'static str> {
    pulsar_world_registry::registered_world_component_classes()
        .filter_map(|name| pulsar_world_registry::component_id_for_class(name).map(|id| (id, name)))
        .collect()
}

/// Scan `scene`, holding its lock a slice at a time.
pub fn scan(scene: &SharedScene) -> SceneIndex {
    let classes = class_names();
    let mut builder = Builder::default();
    builder.index.revision = scene.read().world.revision();
    let mut cursor = (0usize, 0usize);
    loop {
        let guard = scene.read();
        if !builder.slice(&guard.world, &classes, &mut cursor, CHUNK) {
            break;
        }
    }
    builder.finish()
}

/// [`scan`] of a world already borrowed.
#[cfg(test)]
pub fn scan_world(world: &World) -> SceneIndex {
    let classes = class_names();
    let mut builder = Builder::default();
    builder.index.revision = world.revision();
    let mut cursor = (0usize, 0usize);
    while builder.slice(world, &classes, &mut cursor, CHUNK) {}
    builder.finish()
}

#[derive(Default)]
struct Builder {
    index: SceneIndex,
    type_index: HashMap<ComponentId, u32>,
    archetype_index: HashMap<u32, u32>,
}

impl Builder {
    /// Read up to `budget` entities from `cursor` (archetype, row). `false`
    /// when the scan is complete.
    fn slice(
        &mut self,
        world: &World,
        classes: &HashMap<ComponentId, &'static str>,
        cursor: &mut (usize, usize),
        mut budget: usize,
    ) -> bool {
        while budget > 0 {
            let Some(archetype) = world.archetypes.get(cursor.0) else {
                return false;
            };
            if cursor.1 >= archetype.entities.len() {
                *cursor = (cursor.0 + 1, 0);
                continue;
            }
            let archetype_ix = self.archetype(world, classes, cursor.0);
            let end = archetype.entities.len().min(cursor.1 + budget);
            for &entity in &archetype.entities[cursor.1..end] {
                // A row that moved between slices may come around again.
                if self.index.by_entity.contains_key(&entity) {
                    continue;
                }
                let row_ix = self.index.entities.len() as u32;
                let row = self.entity_row(world, entity, archetype_ix);
                self.index.entities.push(row);
                self.index.by_entity.insert(entity, row_ix);
                let archetype_row = &mut self.index.archetypes[archetype_ix as usize];
                archetype_row.entities.push(row_ix);
            }
            budget -= end - cursor.1;
            cursor.1 = end;
        }
        true
    }

    /// The archetype's row, made on its first slice.
    fn archetype(
        &mut self,
        world: &World,
        classes: &HashMap<ComponentId, &'static str>,
        position: usize,
    ) -> u32 {
        let archetype = &world.archetypes[position];
        if let Some(&index) = self.archetype_index.get(&archetype.id.0) {
            return index;
        }
        let first = archetype.entities.first().copied();
        let types: Vec<u32> = archetype
            .key
            .0
            .iter()
            .map(|&id| {
                *self.type_index.entry(id).or_insert_with(|| {
                    let full = pulsar_scenedb::component::type_name(id);
                    let size = first
                        .and_then(|entity| world.get_dyn(entity, id))
                        .map_or(0, std::mem::size_of_val);
                    self.index.types.push(TypeRow {
                        id,
                        name: format::short_type(full).into(),
                        full,
                        class: classes.get(&id).copied(),
                        size,
                        count: 0,
                    });
                    self.index.types.len() as u32 - 1
                })
            })
            .collect();
        let index = self.index.archetypes.len() as u32;
        self.index.archetypes.push(ArchetypeRow {
            id: archetype.id.0,
            types,
            entities: Vec::new(),
        });
        self.archetype_index.insert(archetype.id.0, index);
        index
    }

    fn entity_row(&self, world: &World, entity: Entity, archetype: u32) -> EntityRow {
        let stable_id: Option<Box<str>> =
            world.get::<StableId>(entity).map(|id| id.0.as_str().into());
        let (kind, label): (EntityKind, Box<str>) =
            if let Some(meta) = world.get::<ComponentMeta>(entity) {
                let owner = world.get::<ComponentOwner>(entity);
                (
                    EntityKind::Instance {
                        owner: owner.map_or(Entity::DANGLING, ComponentOwner::entity),
                        enabled: owner.is_none_or(ComponentOwner::is_enabled),
                    },
                    meta.class_name.as_str().into(),
                )
            } else {
                let kind = if stable_id.is_some() {
                    EntityKind::Object
                } else {
                    EntityKind::Other
                };
                let label = match world.get::<Name>(entity) {
                    Some(name) if !name.0.is_empty() => name.0.as_str().into(),
                    _ => match &stable_id {
                        Some(id) => id.clone(),
                        None => self.stand_in_label(archetype),
                    },
                };
                (kind, label)
            };
        let haystack = format!(
            "{} {} {}",
            label,
            stable_id.as_deref().unwrap_or(""),
            format::entity(entity)
        )
        .to_lowercase()
        .into();
        EntityRow {
            entity,
            archetype,
            label,
            kind,
            parent: world.get::<Parent>(entity).map(|parent| parent.0),
            order: world.get::<SiblingIndex>(entity).map_or(0, |order| order.0),
            haystack,
        }
    }

    /// What an entity with no name is called: its component types, by the
    /// first name in order (component ids are assigned in no fixed order).
    fn stand_in_label(&self, archetype: u32) -> Box<str> {
        let types = &self.index.archetypes[archetype as usize].types;
        let first = types
            .iter()
            .map(|&type_ix| &*self.index.types[type_ix as usize].name)
            .min();
        match (first, types.len()) {
            (None, _) => "(no components)".into(),
            (Some(first), 1) => format!("({first})").into(),
            (Some(first), count) => format!("({first} +{})", count - 1).into(),
        }
    }

    fn finish(mut self) -> SceneIndex {
        let index = &mut self.index;
        for archetype in &index.archetypes {
            for &type_ix in &archetype.types {
                index.types[type_ix as usize].count += archetype.entities.len();
            }
        }

        // Types by name: renumber the archetypes' references to match.
        let mut order: Vec<u32> = (0..index.types.len() as u32).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&index.types[a as usize], &index.types[b as usize]);
            a.name.cmp(&b.name).then(a.full.cmp(b.full))
        });
        let mut renumber = vec![0u32; order.len()];
        for (new, &old) in order.iter().enumerate() {
            renumber[old as usize] = new as u32;
        }
        index.types = order
            .iter()
            .map(|&old| index.types[old as usize].clone())
            .collect();
        for archetype in &mut index.archetypes {
            for type_ix in &mut archetype.types {
                *type_ix = renumber[*type_ix as usize];
            }
            archetype.types.sort_unstable();
        }
        index.archetypes.sort_by_key(|archetype| archetype.id);
        for (archetype_ix, archetype) in index.archetypes.iter().enumerate() {
            for &entity_ix in &archetype.entities {
                index.entities[entity_ix as usize].archetype = archetype_ix as u32;
            }
        }

        // The object hierarchy.
        for (row_ix, row) in index.entities.iter().enumerate() {
            if row.kind != EntityKind::Object {
                continue;
            }
            index.object_count += 1;
            let parent = row
                .parent
                .and_then(|parent| index.by_entity.get(&parent))
                .filter(|&&parent| index.entities[parent as usize].kind == EntityKind::Object);
            match parent {
                Some(&parent) => index
                    .children
                    .entry(parent)
                    .or_default()
                    .push(row_ix as u32),
                None => index.roots.push(row_ix as u32),
            }
        }
        let entities = &index.entities;
        let by_sibling = |list: &mut Vec<u32>| {
            list.sort_by_key(|&ix| {
                let row = &entities[ix as usize];
                (row.order, row.entity.bits())
            });
        };
        by_sibling(&mut index.roots);
        index.children.values_mut().for_each(by_sibling);
        self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Marker;
    #[allow(dead_code)]
    struct Value(u64);

    fn object(world: &mut World, id: &str, name: &str, order: u64) -> Entity {
        let entity = world.spawn();
        world.insert(entity, StableId(id.into()));
        world.insert(entity, Name(name.into()));
        world.insert(entity, SiblingIndex(order));
        entity
    }

    #[test]
    fn a_scan_lists_entities_types_and_the_object_tree() {
        let mut world = World::new();
        let second = object(&mut world, "b", "Second", 2);
        let first = object(&mut world, "a", "First", 1);
        let child = object(&mut world, "c", "Child", 0);
        world.insert(child, Parent(first));
        let plain = world.spawn();
        world.insert(plain, Value(7));
        world.insert(plain, Marker);

        let index = scan_world(&world);
        assert_eq!(index.entities.len(), 4);
        assert_eq!(index.object_count, 3);
        let roots: Vec<Entity> = index
            .roots
            .iter()
            .map(|&ix| index.entities[ix as usize].entity)
            .collect();
        assert_eq!(roots, [first, second]);
        let first_ix = index.by_entity[&first];
        assert_eq!(index.children[&first_ix], [index.by_entity[&child]]);

        let value = index.types.iter().find(|t| &*t.name == "Value").unwrap();
        assert_eq!((value.count, value.size), (1, 8));
        let names = index.types.iter().find(|t| &*t.name == "Name").unwrap();
        assert_eq!(names.count, 3);
        assert_eq!(index.entities_with(names.id).len(), 3);
        assert_eq!(index.entity(plain).unwrap().kind, EntityKind::Other);
        assert_eq!(&*index.entity(plain).unwrap().label, "(Marker +1)");

        // Every archetype's references name the right rows after sorting.
        for (archetype_ix, archetype) in index.archetypes.iter().enumerate() {
            for &ix in &archetype.entities {
                let row = &index.entities[ix as usize];
                assert_eq!(row.archetype as usize, archetype_ix);
                let mut has: Vec<ComponentId> = world.component_ids(row.entity).collect();
                let mut listed: Vec<ComponentId> = archetype
                    .types
                    .iter()
                    .map(|&t| index.types[t as usize].id)
                    .collect();
                has.sort_unstable();
                listed.sort_unstable();
                assert_eq!(has, listed);
            }
        }
    }

    #[test]
    fn a_scan_in_small_slices_misses_nothing() {
        let mut world = World::new();
        for index in 0..100 {
            object(&mut world, &format!("o{index}"), "Object", index);
        }
        let classes = HashMap::new();
        let mut builder = Builder::default();
        let mut cursor = (0, 0);
        let mut slices = 0;
        while builder.slice(&world, &classes, &mut cursor, 7) {
            slices += 1;
        }
        assert!(slices >= 100 / 7);
        assert_eq!(builder.finish().entities.len(), 100);
    }
}
