use crate::component::{Column, Component, ComponentId, ErasedColumn};
use crate::entity::Entity;

/// Opaque index into [`World::archetypes`](crate::World).
///
/// ID 0 is reserved for the empty archetype (entities with no components).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArchetypeId(pub u32);

impl ArchetypeId {
    /// The empty-archetype sentinel (ID 0).
    ///
    /// Every `World` starts with one empty archetype.  Newly spawned entities
    /// live here until a component is inserted.
    pub const EMPTY: ArchetypeId = ArchetypeId(0);
}

/// A sorted, deduplicated set of [`ComponentId`]s that uniquely identifies an
/// archetype.
///
/// Archetypes with the same `ArchetypeKey` share the same column layout and
/// are deduplicated by [`World`](crate::World).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArchetypeKey(pub Vec<ComponentId>);

impl ArchetypeKey {
    pub fn new(mut types: Vec<ComponentId>) -> Self {
        types.sort_unstable();
        types.dedup();
        Self(types)
    }

    pub fn with<T: Component>(&self) -> Self {
        let cid = crate::component::component_id::<T>();
        let mut ids = self.0.clone();
        if !ids.contains(&cid) {
            ids.push(cid);
        }
        Self::new(ids)
    }

    pub fn without<T: Component>(&self) -> Self {
        let cid = crate::component::component_id::<T>();
        let ids: Vec<_> = self.0.iter().copied().filter(|c| *c != cid).collect();
        Self(ids)
    }

    pub fn contains<T: Component>(&self) -> bool {
        let cid = crate::component::component_id::<T>();
        self.0.contains(&cid)
    }

    /// Returns true if `self` has at least all the component types in `other`.
    pub fn superset_of(&self, other: &[ComponentId]) -> bool {
        other.iter().all(|c| self.0.contains(c))
    }
}

/// A group of entities that share the exact same set of component types.
///
/// Entities within an archetype are stored in parallel arrays:
/// - `columns[ComponentId]` â†’ `Vec<T>` for each component type
/// - `entities` â†’ `Vec<Entity>`
///
/// Column data is accessed by row index, which is the same across all columns
/// and the entity vec.  Entity removal uses `swap_remove` â€” the last entity
/// is moved into the vacated slot and its row is updated in the slot map.
pub struct Archetype {
    pub id: ArchetypeId,
    pub key: ArchetypeKey,
    /// Pre-computed, immutable list of this archetype's component IDs.
    /// Populated once in [`Archetype::new`] and never modified.  Used by
    /// the migration routines to iterate columns without heap-allocating
    /// intermediate vectors.
    pub(crate) active_cids: Vec<ComponentId>,
    /// Columns indexed by `ComponentId.0 as usize`.  A `None` slot means the
    /// archetype does not contain that component type.  Dense indexing avoids
    /// hashing overhead on the query path.
    pub columns: Vec<Option<Box<dyn ErasedColumn>>>,
    pub entities: Vec<Entity>,
    /// Bitmask over the first 64 component IDs for fast archetype
    /// filtering during queries.  Bit `i` is set if component ID `i+1`
    /// is present.
    pub mask: u64,
}

impl Archetype {
    pub(crate) fn new_empty(id: ArchetypeId) -> Self {
        Self {
            id,
            key: ArchetypeKey(vec![]),
            active_cids: Vec::new(),
            columns: Vec::new(),
            entities: Vec::new(),
            mask: 0,
        }
    }

    pub(crate) fn new(id: ArchetypeId, key: ArchetypeKey) -> Self {
        let mask = key
            .0
            .iter()
            .map(|c| c.0)
            .filter(|&i| i <= 64)
            .fold(0u64, |m, i| m | (1u64 << (i - 1)));
        let active_cids = key.0.clone();
        Self {
            id,
            key,
            active_cids,
            columns: Vec::new(),
            entities: Vec::new(),
            mask,
        }
    }

    /// Number of entities in this archetype.
    #[inline]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Returns `true` if this archetype has no entities.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Register a column for component type `T`.  No-op if already registered.
    pub(crate) fn register_column<T: Component>(&mut self) {
        let cid = crate::component::component_id::<T>();
        let idx = cid.0 as usize;
        for _ in self.columns.len()..=idx {
            self.columns.push(None);
        }
        if self.columns[idx].is_none() {
            self.columns[idx] = Some(Box::new(Column::<T>::new()));
        }
    }

    #[inline]
    pub(crate) fn column<T: Component>(&self) -> &Column<T> {
        let cid = crate::component::component_id::<T>();
        self.columns[cid.0 as usize]
            .as_ref()
            .unwrap_or_else(|| panic!("column not registered for cid={:?}", cid))
            .as_any()
            .downcast_ref::<Column<T>>()
            .expect("column type mismatch")
    }

    #[inline]
    pub(crate) fn column_mut<T: Component>(&mut self) -> &mut Column<T> {
        let cid = crate::component::component_id::<T>();
        let erased = self.columns[cid.0 as usize]
            .as_mut()
            .unwrap_or_else(|| panic!("column not registered for cid={:?}", cid));
        // Debug check: verify the stored TypeId matches T
        assert_eq!(
            ErasedColumn::type_id(erased.as_ref()),
            std::any::TypeId::of::<T>(),
            "column type mismatch for cid={:?}: stored={:?} expected={:?}",
            cid,
            ErasedColumn::type_id(erased.as_ref()),
            std::any::TypeId::of::<T>(),
        );
        erased
            .as_any_mut()
            .downcast_mut::<Column<T>>()
            .unwrap_or_else(|| unreachable!("downcast should match after type_id check"))
    }

    pub(crate) fn has_column<T: Component>(&self) -> bool {
        let cid = crate::component::component_id::<T>();
        let idx = cid.0 as usize;
        idx < self.columns.len() && self.columns[idx].is_some()
    }

    /// True if this archetype has all of the components identified by `ids`.
    #[inline]
    pub(crate) fn has_columns(&self, ids: &[ComponentId]) -> bool {
        for &cid in ids {
            let idx = cid.0 as usize;
            // Fast path: check the bitmask first (only valid for cid â‰¤ 64).
            if cid.0 <= 64 && (self.mask & (1u64 << (cid.0 - 1))) == 0 {
                return false;
            }
            // Verify the Vec slot is populated.
            if idx >= self.columns.len() || self.columns[idx].is_none() {
                return false;
            }
        }
        true
    }

    /// Get an erased column by ComponentId.
    #[inline]
    pub(crate) fn get_erased(&self, cid: ComponentId) -> Option<&dyn ErasedColumn> {
        self.columns
            .get(cid.0 as usize)
            .and_then(|c| c.as_deref())
    }

    /// Get a mutable erased column by ComponentId.
    #[inline]
    pub(crate) fn get_erased_mut(&mut self, cid: ComponentId) -> Option<&mut dyn ErasedColumn> {
        self.columns
            .get_mut(cid.0 as usize)
            .and_then(|c| c.as_mut())
            .map(|b| b.as_mut())
    }

    /// Remove the entity at `row` (swap-remove).  Returns the swapped-in
    /// entity, or `None` if the removed entity was the last one.
    pub(crate) fn remove_row(&mut self, row: usize) -> Option<Entity> {
        let last = self.entities.len() - 1;
        for col in self.columns.iter_mut().filter_map(|c| c.as_mut()) {
            // SAFETY: row is guaranteed < self.len()
            let ptr = unsafe { col.swap_remove_erased(row) };
            // SAFETY: the returned pointer is a valid heap allocation for this
            // column's concrete type.
            unsafe { col.drop_erased(ptr) };
        }
        let moved = if row < last {
            // NOTE: swap_remove returns the *removed* element â€” we want the
            // one that was *moved into* `row` (which is now at position `row`).
            self.entities.swap_remove(row);
            Some(self.entities[row])
        } else {
            self.entities.pop();
            None
        };
        moved
    }
}