use crate::archetype::{Archetype, ArchetypeId, ArchetypeKey};
use crate::component::{Column, Component, ErasedColumn};
use crate::entity::{Entity, EntitySlot};
use ahash::AHashMap;
use std::any::TypeId;

/// The central ECS store.
///
/// # Layout
/// - `entity_slots`: flat pool indexed by `Entity::index()`.
/// - `archetypes`: dense `Vec<Archetype>` indexed by `ArchetypeId`.
/// - `archetype_index`: `ArchetypeKey → ArchetypeId` for O(log n) archetype lookup.
///
/// # Guarantees
/// - Entities are invalidated on `despawn` via a 32-bit generation counter.
/// - Component migrations (insert/remove) preserve all existing component values.
pub struct World {
    pub(crate) entity_slots: Vec<EntitySlot>,
    pub(crate) free_slots: Vec<u32>,
    pub(crate) archetypes: Vec<Archetype>,
    pub(crate) archetype_index: AHashMap<ArchetypeKey, ArchetypeId>,
}

impl World {
    /// Create an empty world with one archetype: the zero-component "empty" archetype.
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

    // ── Entity lifetime ──────────────────────────────────────────────────────

    /// Allocate a new entity with no components.
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

    /// Destroy an entity and drop all its components.
    /// Returns `false` if the entity was already dead.
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

    // ── Component access ─────────────────────────────────────────────────────

    /// Insert (or overwrite) a component on `entity`, migrating it to the correct archetype.
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) {
        assert!(self.is_alive(entity), "insert on dead entity {entity}");

        let (old_arch_id, old_row) = {
            let s = &self.entity_slots[entity.index() as usize];
            (s.archetype, s.row as usize)
        };

        // Fast path: entity already has this component — just overwrite.
        if self.archetypes[old_arch_id.0 as usize].has_column::<T>() {
            let col = self.archetypes[old_arch_id.0 as usize].column_mut::<T>();
            col.data[old_row] = value;
            return;
        }

        // Compute the destination archetype key.
        let new_key = self.archetypes[old_arch_id.0 as usize].key.with::<T>();
        let new_arch_id = self.get_or_create_archetype(new_key);

        // Migrate: collect all component values from the old archetype row as
        // owning pointers, then push them into the new archetype.
        self.migrate_row(entity, old_arch_id, old_row, new_arch_id, None::<fn(TypeId)>);

        // Push the new component T.
        self.ensure_column::<T>(new_arch_id);
        self.archetypes[new_arch_id.0 as usize]
            .column_mut::<T>()
            .data
            .push(value);

        let new_row = (self.archetypes[new_arch_id.0 as usize].entities.len()) as u32;
        self.archetypes[new_arch_id.0 as usize].entities.push(entity);

        let slot = &mut self.entity_slots[entity.index() as usize];
        slot.archetype = new_arch_id;
        slot.row = new_row;
    }

    /// Remove a component from `entity`, migrating it to the smaller archetype.
    /// Returns the removed value, or `None` if the entity didn't have it.
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

        // Extract the T value before migrating.
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

        // Migrate remaining columns (skip T — already removed above).
        self.migrate_row_skip(entity, old_arch_id, old_row, new_arch_id, t_id);

        Some(removed_val)
    }

    /// Get a shared reference to a component on `entity`.
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

    /// Get a mutable reference to a component on `entity`.
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

    // ── Archetype management ──────────────────────────────────────────────────

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

    /// Move all component values from `old_arch[old_row]` into `new_arch`,
    /// skipping the component identified by `skip_type` (already extracted by caller).
    fn migrate_row_skip(
        &mut self,
        entity: Entity,
        old_arch_id: ArchetypeId,
        old_row: usize,
        new_arch_id: ArchetypeId,
        skip_type: TypeId,
    ) {
        // Collect (TypeId, ptr) for every column except the skipped one.
        let type_ids: Vec<TypeId> = self.archetypes[old_arch_id.0 as usize]
            .columns
            .keys()
            .filter(|&&t| t != skip_type)
            .copied()
            .collect();

        // For each remaining type: swap-remove from old, push into new.
        for type_id in type_ids {
            // swap_remove on the old column
            let ptr = unsafe {
                self.archetypes[old_arch_id.0 as usize]
                    .columns
                    .get_mut(&type_id)
                    .unwrap()
                    .swap_remove_erased(old_row)
            };

            // Ensure the destination column exists.
            if !self.archetypes[new_arch_id.0 as usize]
                .columns
                .contains_key(&type_id)
            {
                let proto = self.archetypes[old_arch_id.0 as usize]
                    .columns[&type_id]
                    .new_empty();
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

        // Update entity records.
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
        self.archetypes[new_arch_id.0 as usize].entities.push(entity);
        let slot = &mut self.entity_slots[entity.index() as usize];
        slot.archetype = new_arch_id;
        slot.row = new_row;
    }

    /// Move all component values from `old_arch[old_row]` into `new_arch`.
    /// The `_filter` parameter is unused here; present to match `migrate_row_skip` signature.
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
                let proto = self.archetypes[old_arch_id.0 as usize]
                    .columns[&type_id]
                    .new_empty();
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

        // The entity record update is done by the caller (insert path) after
        // pushing the new T — so here we only fix up the old archetype side.
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
        // Caller is responsible for pushing entity into new_arch and updating slot.
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
