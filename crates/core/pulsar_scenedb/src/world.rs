use crate::archetype::{Archetype, ArchetypeId, ArchetypeKey};
use crate::component::{Column, Component, ComponentId, ErasedColumn};
use crate::entity::{Entity, EntitySlot};
use ahash::AHashMap;

/// The central ECS store: owns all entities, their component data, and the
/// archetype graph.
///
/// # Entity lifecycle
///
/// 1. [`World::spawn`] allocates a slot and places the entity in the empty
///    archetype (no components).
/// 2. [`World::insert`] adds a component, migrating the entity to a new
///    archetype.
/// 3. [`World::remove`] strips a component, migrating back.
/// 4. [`World::despawn`] frees the slot and swap-removes the entity from its
///    archetype.
///
/// # Queries
///
/// Use [`World::query`] to iterate entities matching a component pattern.
/// Queries scan all archetypes, using the `u64` bitmask to skip non-matching
/// archetypes in constant time.
pub struct World {
    pub entity_slots: Vec<EntitySlot>,
    pub free_slots: Vec<u32>,
    pub archetypes: Vec<Archetype>,
    pub archetype_index: AHashMap<ArchetypeKey, ArchetypeId>,
}

impl World {
    /// Create an empty world with one empty archetype and no entities.
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

    /// Debug assertion: every archetype's column lengths must equal its entity
    /// count.  Panics on the first mismatch.  Compiled out in release builds
    /// (the loop body becomes a no-op).
    #[inline]
    pub fn assert_archetype_consistency(&self) {
        #[cfg(debug_assertions)]
        for arch in &self.archetypes {
            let elen = arch.entities.len();
            for (cidx, col) in arch.columns.iter().enumerate() {
                if let Some(c) = col {
                    assert_eq!(
                        c.len(),
                        elen,
                        "ArchetypeId({}) column[{}] len {} != entities.len {} (key={:?})",
                        arch.id.0,
                        cidx,
                        c.len(),
                        elen,
                        arch.key.0,
                    );
                }
            }
        }
    }

    // â”€â”€ Entity lifecycle â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// Pre-allocate storage for `count` entities.  Call before a batch spawn
    /// loop to avoid repeated capacity-doubling reallocations of the slot vec
    /// and the empty archetype's entity vec.
    pub fn reserve_entities(&mut self, count: u32) {
        self.entity_slots.reserve(count as usize);
        self.archetypes[ArchetypeId::EMPTY.0 as usize]
            .entities
            .reserve(count as usize);
    }

    /// Allocate a new entity in the empty archetype.
    ///
    /// Recycles a free slot if one is available; otherwise extends the slot
    /// vec.  The returned handle includes a generation counter that allows
    /// [`is_alive`](Self::is_alive) to detect stale handles after despawn.
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

    /// Remove an entity and all its components from the world.
    ///
    /// Returns `false` if the entity was already dead (generation mismatch or
    /// out-of-bounds index).
    ///
    /// The entity's slot is recycled: the generation is incremented and the
    /// index is pushed onto the free list.  The entity's data is
    /// swap-removed from its archetype.
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

    /// Returns `true` if `entity` is still alive.
    ///
    /// Checks that the slot exists (index in bounds) and that the stored
    /// generation matches the entity handle's generation â€” meaning the slot
    /// hasn't been recycled since the handle was created.
    #[inline]
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entity_slots
            .get(entity.index() as usize)
            .map(|s| s.generation == entity.generation())
            .unwrap_or(false)
    }

    // â”€â”€ Component helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// Fast path: check whether archetype `arch_id` has a column at `cid`.
    #[inline]
    fn has_column_id(arch: &Archetype, cid: ComponentId) -> bool {
        let idx = cid.0 as usize;
        idx < arch.columns.len() && arch.columns[idx].is_some()
    }

    /// Get a mutable reference to the `ErasedColumn` at `cid` in `arch`.
    #[inline]
    fn get_erased_mut(arch: &mut Archetype, cid: ComponentId) -> Option<&mut Box<dyn ErasedColumn>> {
        arch.columns.get_mut(cid.0 as usize).and_then(|c| c.as_mut())
    }

    /// Get a shared reference to the `ErasedColumn` at `cid` in `arch`.
    #[inline]
    fn get_erased(arch: &Archetype, cid: ComponentId) -> Option<&Box<dyn ErasedColumn>> {
        arch.columns.get(cid.0 as usize).and_then(|c| c.as_ref())
    }

    /// Ensure the columns vec is large enough for `cid`, then set it.
    #[inline]
    fn set_column(arch: &mut Archetype, cid: ComponentId, col: Box<dyn ErasedColumn>) {
        let idx = cid.0 as usize;
        for _ in arch.columns.len()..=idx {
            arch.columns.push(None);
        }
        arch.columns[idx] = Some(col);
    }

    /// Collect all CIDs that have a column in this archetype (for migration).
    fn collect_cids(arch: &Archetype) -> Vec<ComponentId> {
        arch.columns
            .iter()
            .enumerate()
            .filter(|(_, col)| col.is_some())
            .map(|(i, _)| ComponentId(i as u32))
            .collect()
    }

    /// Collect all CIDs except `skip` (for migration skip).
    fn collect_cids_skip(arch: &Archetype, skip: ComponentId) -> Vec<ComponentId> {
        arch.columns
            .iter()
            .enumerate()
            .filter(|(i, col)| col.is_some() && ComponentId(*i as u32) != skip)
            .map(|(i, _)| ComponentId(i as u32))
            .collect()
    }

    // â”€â”€ Component operations â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// Add a component to an entity, migrating it to a new archetype if needed.
    ///
    /// If the entity already has a component of type `T`, the value is
    /// overwritten in place (no migration).  Otherwise the entity is moved to
    /// an archetype that includes `T`, preserving all existing component data.
    ///
    /// # Panics
    ///
    /// Panics if `entity` is dead.
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) {
        assert!(self.is_alive(entity), "insert on dead entity {entity}");

        let (old_arch_id, old_row) = {
            let s = &self.entity_slots[entity.index() as usize];
            (s.archetype, s.row as usize)
        };

        // In-place update: entity already has this component in this archetype.
        let cid = crate::component::component_id::<T>();
        if Self::has_column_id(&self.archetypes[old_arch_id.0 as usize], cid) {
            let col = self.archetypes[old_arch_id.0 as usize].column_mut::<T>();
            col.data[old_row] = value;
            return;
        }

        // Build the destination archetype key and ensure it exists.
        let new_key = self.archetypes[old_arch_id.0 as usize].key.with::<T>();
        let new_arch_id = self.get_or_create_archetype(new_key);

        // Ensure Column<T> exists in the destination (may be empty).
        let new_arch = &mut self.archetypes[new_arch_id.0 as usize];
        let idx = cid.0 as usize;
        if let Some(existing) = new_arch.columns.get(idx).and_then(|c| c.as_ref()) {
            debug_assert_eq!(
                ErasedColumn::type_id(existing.as_ref()),
                std::any::TypeId::of::<T>(),
                "insert column type collision at {:?}",
                cid,
            );
        } else {
            Self::set_column(new_arch, cid, Box::new(Column::<T>::new()));
        }

        // Phase 1: push entity + migrate all existing components.
        // migrate_row pushes the entity to the destination first, then
        // transfers every column from the source, then updates all slots.
        self.migrate_row(entity, old_arch_id, old_row, new_arch_id);

        // Phase 2: push the new value.  The destination entity vec has
        // already grown by one, so this keeps all column lengths in sync.
        let new_arch = &mut self.archetypes[new_arch_id.0 as usize];
        new_arch.columns[idx]
            .as_mut()
            .unwrap()
            .as_any_mut()
            .downcast_mut::<Column<T>>()
            .unwrap()
            .data
            .push(value);
    }

    /// Remove a component from an entity, returning its value.
    ///
    /// The entity is migrated to an archetype without `T`.  All other
    /// components are preserved.
    ///
    /// Returns `None` if the entity is dead or does not have component `T`.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        if !self.is_alive(entity) {
            return None;
        }
        let (old_arch_id, old_row) = {
            let s = &self.entity_slots[entity.index() as usize];
            (s.archetype, s.row as usize)
        };
        let cid = crate::component::component_id::<T>();
        if !Self::has_column_id(&self.archetypes[old_arch_id.0 as usize], cid) {
            return None;
        }

        // Pull the value out of the column.
        let removed_ptr = unsafe {
            Self::get_erased_mut(&mut self.archetypes[old_arch_id.0 as usize], cid)
                .unwrap()
                .swap_remove_erased(old_row)
        };
        // SAFETY: we know the concrete type from the generic.
        let removed_val = unsafe { *Box::from_raw(removed_ptr as *mut T) };

        // Build the destination key WITHOUT this component.
        let new_key = self.archetypes[old_arch_id.0 as usize]
            .key
            .without::<T>();
        let new_arch_id = self.get_or_create_archetype(new_key);

        // Migrate everything except the removed component.
        // migrate_row_skip pushes the entity first, migrates all columns
        // except the skipped one, then updates all slots.
        self.migrate_row_skip(entity, old_arch_id, old_row, new_arch_id, cid);

        Some(removed_val)
    }

    /// Returns a shared reference to component `T` on `entity`, if present.
    #[inline]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        if !self.is_alive(entity) {
            return None;
        }
        let s = &self.entity_slots[entity.index() as usize];
        let arch = &self.archetypes[s.archetype.0 as usize];
        let cid = crate::component::component_id::<T>();
        Self::get_erased(arch, cid).and_then(|c| {
            c.as_any()
                .downcast_ref::<Column<T>>()
                .map(|col| &col.data[s.row as usize])
        })
    }

    /// Returns a mutable reference to component `T` on `entity`, if present.
    #[inline]
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.is_alive(entity) {
            return None;
        }
        let (arch_id, row) = {
            let s = &self.entity_slots[entity.index() as usize];
            (s.archetype, s.row as usize)
        };
        let cid = crate::component::component_id::<T>();
        Self::get_erased_mut(&mut self.archetypes[arch_id.0 as usize], cid).and_then(|c| {
            c.as_any_mut()
                .downcast_mut::<Column<T>>()
                .map(|col| &mut col.data[row])
        })
    }

    // â”€â”€ Archetype graph â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    pub(crate) fn get_or_create_archetype(&mut self, key: ArchetypeKey) -> ArchetypeId {
        if let Some(&id) = self.archetype_index.get(&key) {
            return id;
        }
        let id = ArchetypeId(self.archetypes.len() as u32);
        self.archetypes.push(Archetype::new(id, key.clone()));
        self.archetype_index.insert(key, id);
        id
    }

    // â”€â”€ Archetype migration â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// Move the entity and all component data from `old_arch_id`/`old_row`
    /// into `new_arch_id`.
    ///
    /// Order of operations (single cohesive window):
    /// 1. Push entity to destination `.entities` (first).
    /// 2. For each component in `active_cids` of the source archetype:
    ///    swap-remove from the source column, ensure the destination column
    ///    exists, and push into it.
    /// 3. Swap-remove the entity from the source archetype and fix the
    ///    swapped-in entity's slot row.
    /// 4. Update the migrated entity's slot.
    ///
    /// The caller is responsible for pushing any *new* component value (not
    /// present in the source archetype) *after* this returns.
    fn migrate_row(
        &mut self,
        entity: Entity,
        old_arch_id: ArchetypeId,
        old_row: usize,
        new_arch_id: ArchetypeId,
    ) {
        // Phase 1: push entity to destination first.
        let new_row = self.archetypes[new_arch_id.0 as usize]
            .entities
            .len() as u32;
        self.archetypes[new_arch_id.0 as usize]
            .entities
            .push(entity);

        // Phase 2: broadcast each source column into the destination using
        // the pre-computed `active_cids` slice (no heap allocation).
        let n = self.archetypes[old_arch_id.0 as usize].active_cids.len();
        for i in 0..n {
            let cid = {
                // isolated immutable borrow â€” released before the mutable one
                let src = &self.archetypes[old_arch_id.0 as usize];
                src.active_cids[i]
            };
            let ptr = unsafe {
                Self::get_erased_mut(&mut self.archetypes[old_arch_id.0 as usize], cid)
                    .unwrap()
                    .swap_remove_erased(old_row)
            };
            if !Self::has_column_id(&self.archetypes[new_arch_id.0 as usize], cid) {
                let proto = Self::get_erased(&self.archetypes[old_arch_id.0 as usize], cid)
                    .unwrap()
                    .new_empty();
                Self::set_column(&mut self.archetypes[new_arch_id.0 as usize], cid, proto);
            }
            unsafe {
                Self::get_erased_mut(&mut self.archetypes[new_arch_id.0 as usize], cid)
                    .unwrap()
                    .push_erased(ptr);
            }
        }

        // Phase 3: remove entity from old archetype; fix swapped-in slot.
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

        // Phase 4: update the migrated entity's slot.
        let slot = &mut self.entity_slots[entity.index() as usize];
        slot.archetype = new_arch_id;
        slot.row = new_row;
    }

    /// Move all components EXCEPT `skip_cid` and push the entity into the
    /// destination archetype.
    ///
    /// Same ordering as [`migrate_row`]: entity first, then columns, then
    /// slot updates.
    fn migrate_row_skip(
        &mut self,
        entity: Entity,
        old_arch_id: ArchetypeId,
        old_row: usize,
        new_arch_id: ArchetypeId,
        skip_cid: ComponentId,
    ) {
        // Phase 1: push entity to destination first.
        let new_row = self.archetypes[new_arch_id.0 as usize]
            .entities
            .len() as u32;
        self.archetypes[new_arch_id.0 as usize]
            .entities
            .push(entity);

        // Phase 2: migrate all columns except `skip_cid`.
        let n = self.archetypes[old_arch_id.0 as usize].active_cids.len();
        for i in 0..n {
            let cid = {
                let src = &self.archetypes[old_arch_id.0 as usize];
                src.active_cids[i]
            };
            if cid == skip_cid {
                continue;
            }
            let ptr = unsafe {
                Self::get_erased_mut(&mut self.archetypes[old_arch_id.0 as usize], cid)
                    .unwrap()
                    .swap_remove_erased(old_row)
            };
            if !Self::has_column_id(&self.archetypes[new_arch_id.0 as usize], cid) {
                let proto = Self::get_erased(&self.archetypes[old_arch_id.0 as usize], cid)
                    .unwrap()
                    .new_empty();
                Self::set_column(&mut self.archetypes[new_arch_id.0 as usize], cid, proto);
            }
            unsafe {
                Self::get_erased_mut(&mut self.archetypes[new_arch_id.0 as usize], cid)
                    .unwrap()
                    .push_erased(ptr);
            }
        }

        // Phase 3: remove entity from old archetype; fix swapped-in slot.
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

        // Phase 4: update the migrated entity's slot.
        let slot = &mut self.entity_slots[entity.index() as usize];
        slot.archetype = new_arch_id;
        slot.row = new_row;
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}