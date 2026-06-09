use crate::archetype::{Archetype, ArchetypeId, ArchetypeKey};
use crate::component::{Column, Component, ComponentId, ErasedColumn};
use crate::entity::{Entity, EntitySlot};
use ahash::AHashMap;

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

    // ── Entity lifecycle ─────────────────────────────────────────────────

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

    // ── Component helpers ────────────────────────────────────────────────

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

    // ── Component operations ─────────────────────────────────────────────

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

        // Build the destination archetype key and ensure it exists, then
        // pre-populate the new Column<T> before migrating existing columns.
        let new_key = self.archetypes[old_arch_id.0 as usize].key.with::<T>();
        let new_arch_id = self.get_or_create_archetype(new_key);

        // Add an empty Column<T> in the destination before migration.
        let new_arch = &mut self.archetypes[new_arch_id.0 as usize];
        let idx = cid.0 as usize;
        if let Some(existing) = new_arch.columns.get(idx).and_then(|c| c.as_ref()) {
            // Debug check: type must match
            debug_assert_eq!(
                ErasedColumn::type_id(existing.as_ref()),
                std::any::TypeId::of::<T>(),
                "insert column type collision at {:?}",
                cid,
            );
        } else {
            Self::set_column(new_arch, cid, Box::new(Column::<T>::new()));
        }
        // Push the new value (column now exists at idx).
        new_arch.columns[idx]
            .as_mut()
            .unwrap()
            .as_any_mut()
            .downcast_mut::<Column<T>>()
            .unwrap()
            .data
            .push(value);

        // Migrate ALL existing components from old → new archetype.
        self.migrate_row(entity, old_arch_id, old_row, new_arch_id);

        // Update slot to point at the new archetype + row.
        let new_row = self.archetypes[new_arch_id.0 as usize]
            .entities
            .len() as u32;
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
        self.migrate_row_skip(entity, old_arch_id, old_row, new_arch_id, cid);

        Some(removed_val)
    }

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

    // ── Archetype graph ─────────────────────────────────────────────────

    pub(crate) fn get_or_create_archetype(&mut self, key: ArchetypeKey) -> ArchetypeId {
        if let Some(&id) = self.archetype_index.get(&key) {
            return id;
        }
        let id = ArchetypeId(self.archetypes.len() as u32);
        self.archetypes.push(Archetype::new(id, key.clone()));
        self.archetype_index.insert(key, id);
        id
    }

    // ── Archetype migration ──────────────────────────────────────────────

    /// Move all components from `old_arch_id`/`old_row` into
    /// `new_arch_id`.  The destination archetype must already have columns
    /// for all the components being moved (or they will be cloned from the
    /// source).
    ///
    /// After migration the entity is removed from the OLD archetype but is
    /// NOT added to the NEW one — the caller is responsible for that.
    fn migrate_row(
        &mut self,
        entity: Entity,
        old_arch_id: ArchetypeId,
        old_row: usize,
        new_arch_id: ArchetypeId,
    ) {
        let cids = Self::collect_cids(&self.archetypes[old_arch_id.0 as usize]);

        for &cid in &cids {
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

        // Fix entity-slot rows for the swap-removed entity in the OLD arch.
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

    /// Move all components EXCEPT `skip_cid`.  Also pushes the entity into
    /// the destination archetype and updates the slot — callers should NOT
    /// repeat that work.
    fn migrate_row_skip(
        &mut self,
        entity: Entity,
        old_arch_id: ArchetypeId,
        old_row: usize,
        new_arch_id: ArchetypeId,
        skip_cid: ComponentId,
    ) {
        let cids = Self::collect_cids_skip(&self.archetypes[old_arch_id.0 as usize], skip_cid);

        for &cid in &cids {
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

        let new_row = self.archetypes[new_arch_id.0 as usize]
            .entities
            .len() as u32;
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
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}