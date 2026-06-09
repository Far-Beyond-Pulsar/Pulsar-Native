use crate::archetype::{Archetype, ArchetypeId, ArchetypeKey};
use crate::component::{Column, Component, ErasedColumn};
use crate::entity::{Entity, EntitySlot};
use ahash::AHashMap;
use std::any::TypeId;

pub struct World {
    pub entity_slots: Vec<EntitySlot>,
    pub free_slots: Vec<u32>,
    pub archetypes: Vec<Archetype>,
    pub archetype_index: AHashMap<ArchetypeKey, ArchetypeId>,
}

impl World {
    pub fn new() -> Self {
        let empty = Archetype::new_empty(ArchetypeId::EMPTY);
        let mut archetype_index = AHashMap::default();
        archetype_index.insert(ArchetypeKey(vec![]), ArchetypeId::EMPTY);
        Self {
            entity_slots: Vec::new(),
            free_slots: Vec::new(),
            archetypes: vec![empty],
            archetype_index,
        }
    }

    pub fn spawn(&mut self) -> Entity {
        let (idx, gen) = if let Some(idx) = self.free_slots.pop() {
            let slot = &mut self.entity_slots[idx as usize];
            slot.generation = slot.generation.wrapping_add(1);
            slot.archetype = ArchetypeId::EMPTY;
            (idx, slot.generation)
        } else {
            let idx = self.entity_slots.len() as u32;
            self.entity_slots.push(EntitySlot::empty(0));
            (idx, 0)
        };

        let entity = Entity::new(idx, gen);
        let empty = &mut self.archetypes[ArchetypeId::EMPTY.0 as usize];
        let row = empty.entities.len() as u32;
        empty.entities.push(entity);
        self.entity_slots[idx as usize].row = row;
        entity
    }

    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        let (arch_id, row) = {
            let s = &self.entity_slots[entity.index() as usize];
            (s.archetype, s.row as usize)
        };
        let swapped = self.archetypes[arch_id.0 as usize].remove_row(row);
        if let Some(moved) = swapped {
            self.entity_slots[moved.index() as usize].row = row as u32;
        }
        let slot = &mut self.entity_slots[entity.index() as usize];
        slot.generation = slot.generation.wrapping_add(1);
        self.free_slots.push(entity.index());
        true
    }

    #[inline]
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entity_slots
            .get(entity.index() as usize)
            .map(|s| s.generation == entity.generation())
            .unwrap_or(false)
    }

    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) {
        assert!(self.is_alive(entity), "insert on dead entity {entity}");

        let (old_arch_id, old_row) = {
            let s = &self.entity_slots[entity.index() as usize];
            (s.archetype, s.row as usize)
        };

        if self.archetypes[old_arch_id.0 as usize].has_column::<T>() {
            let col = self.archetypes[old_arch_id.0 as usize].column_mut::<T>();
            col.data[old_row] = value;
            return;
        }

        let new_key = self.archetypes[old_arch_id.0 as usize].key.with::<T>();
        let new_arch_id = self.get_or_create_archetype(new_key);

        self.migrate_row(
            entity,
            old_arch_id,
            old_row,
            new_arch_id,
            None::<fn(TypeId)>,
        );

        self.ensure_column::<T>(new_arch_id);
        self.archetypes[new_arch_id.0 as usize]
            .column_mut::<T>()
            .data
            .push(value);

        let new_row = (self.archetypes[new_arch_id.0 as usize].entities.len()) as u32;
        self.archetypes[new_arch_id.0 as usize]
            .entities
            .push(entity);

        let slot = &mut self.entity_slots[entity.index() as usize];
        slot.archetype = new_arch_id;
        slot.row = new_row;
    }

    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        if !self.is_alive(entity) {
            return None;
        }
        let (old_arch_id, old_row) = {
            let s = &self.entity_slots[entity.index() as usize];
            (s.archetype, s.row as usize)
        };
        if !self.archetypes[old_arch_id.0 as usize].has_column::<T>() {
            return None;
        }

        let t_id = TypeId::of::<T>();
        let removed_ptr = unsafe {
            self.archetypes[old_arch_id.0 as usize]
                .columns
                .get_mut(&t_id)
                .unwrap()
                .swap_remove_erased(old_row)
        };
        let removed_val = unsafe { *Box::from_raw(removed_ptr as *mut T) };

        let new_key = self.archetypes[old_arch_id.0 as usize].key.without::<T>();
        let new_arch_id = self.get_or_create_archetype(new_key);

        self.migrate_row_skip(entity, old_arch_id, old_row, new_arch_id, t_id);

        Some(removed_val)
    }

    #[inline]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        if !self.is_alive(entity) {
            return None;
        }
        let s = &self.entity_slots[entity.index() as usize];
        let arch = &self.archetypes[s.archetype.0 as usize];
        arch.columns
            .get(&TypeId::of::<T>())
            .and_then(|c| c.as_any().downcast_ref::<Column<T>>())
            .map(|c| &c.data[s.row as usize])
    }

    #[inline]
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.is_alive(entity) {
            return None;
        }
        let (arch_id, row) = {
            let s = &self.entity_slots[entity.index() as usize];
            (s.archetype, s.row as usize)
        };
        self.archetypes[arch_id.0 as usize]
            .columns
            .get_mut(&TypeId::of::<T>())
            .and_then(|c| c.as_any_mut().downcast_mut::<Column<T>>())
            .map(|c| &mut c.data[row])
    }

    pub(crate) fn get_or_create_archetype(&mut self, key: ArchetypeKey) -> ArchetypeId {
        if let Some(&id) = self.archetype_index.get(&key) {
            return id;
        }
        let id = ArchetypeId(self.archetypes.len() as u32);
        self.archetypes.push(Archetype::new(id, key.clone()));
        self.archetype_index.insert(key, id);
        id
    }

    fn ensure_column<T: Component>(&mut self, arch_id: ArchetypeId) {
        let arch = &mut self.archetypes[arch_id.0 as usize];
        arch.columns
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Column::<T>::new()));
    }

    fn migrate_row_skip(
        &mut self,
        entity: Entity,
        old_arch_id: ArchetypeId,
        old_row: usize,
        new_arch_id: ArchetypeId,
        skip_type: TypeId,
    ) {
        let type_ids: Vec<TypeId> = self.archetypes[old_arch_id.0 as usize]
            .columns
            .keys()
            .filter(|&&t| t != skip_type)
            .copied()
            .collect();

        for type_id in type_ids {
            let ptr = unsafe {
                self.archetypes[old_arch_id.0 as usize]
                    .columns
                    .get_mut(&type_id)
                    .unwrap()
                    .swap_remove_erased(old_row)
            };

            if !self.archetypes[new_arch_id.0 as usize]
                .columns
                .contains_key(&type_id)
            {
                let proto = self.archetypes[old_arch_id.0 as usize].columns[&type_id].new_empty();
                self.archetypes[new_arch_id.0 as usize]
                    .columns
                    .insert(type_id, proto);
            }

            unsafe {
                self.archetypes[new_arch_id.0 as usize]
                    .columns
                    .get_mut(&type_id)
                    .unwrap()
                    .push_erased(ptr);
            }
        }

        let new_row = self.archetypes[new_arch_id.0 as usize].entities.len() as u32;
        let moved = {
            let old_arch = &mut self.archetypes[old_arch_id.0 as usize];
            old_arch.entities.swap_remove(old_row);
            if old_row < old_arch.entities.len() {
                Some(old_arch.entities[old_row])
            } else {
                None
            }
        };
        if let Some(m) = moved {
            self.entity_slots[m.index() as usize].row = old_row as u32;
        }
        self.archetypes[new_arch_id.0 as usize]
            .entities
            .push(entity);
        let slot = &mut self.entity_slots[entity.index() as usize];
        slot.archetype = new_arch_id;
        slot.row = new_row;
    }

    fn migrate_row<F: Fn(TypeId)>(
        &mut self,
        entity: Entity,
        old_arch_id: ArchetypeId,
        old_row: usize,
        new_arch_id: ArchetypeId,
        _filter: Option<F>,
    ) {
        let type_ids: Vec<TypeId> = self.archetypes[old_arch_id.0 as usize]
            .columns
            .keys()
            .copied()
            .collect();

        for type_id in type_ids {
            let ptr = unsafe {
                self.archetypes[old_arch_id.0 as usize]
                    .columns
                    .get_mut(&type_id)
                    .unwrap()
                    .swap_remove_erased(old_row)
            };
            if !self.archetypes[new_arch_id.0 as usize]
                .columns
                .contains_key(&type_id)
            {
                let proto = self.archetypes[old_arch_id.0 as usize].columns[&type_id].new_empty();
                self.archetypes[new_arch_id.0 as usize]
                    .columns
                    .insert(type_id, proto);
            }
            unsafe {
                self.archetypes[new_arch_id.0 as usize]
                    .columns
                    .get_mut(&type_id)
                    .unwrap()
                    .push_erased(ptr);
            }
        }

        let moved = {
            let old_arch = &mut self.archetypes[old_arch_id.0 as usize];
            old_arch.entities.swap_remove(old_row);
            if old_row < old_arch.entities.len() {
                Some(old_arch.entities[old_row])
            } else {
                None
            }
        };
        if let Some(m) = moved {
            self.entity_slots[m.index() as usize].row = old_row as u32;
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
