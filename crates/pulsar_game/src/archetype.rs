use crate::component::{Column, Component, ErasedColumn};
use crate::entity::Entity;
use std::any::TypeId;
use std::collections::HashMap;

/// Opaque, stable index into `World::archetypes`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArchetypeId(pub(crate) u32);

impl ArchetypeId {
    /// The archetype that holds entities with zero components.
    pub const EMPTY: ArchetypeId = ArchetypeId(0);
}

/// The sorted list of component `TypeId`s that defines an archetype.
///
/// Sorting ensures that `(A, B)` and `(B, A)` map to the same archetype.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArchetypeKey(pub(crate) Vec<TypeId>);

impl ArchetypeKey {
    pub fn new(mut types: Vec<TypeId>) -> Self {
        types.sort_unstable();
        types.dedup();
        Self(types)
    }

    pub fn with<T: Component>(&self) -> Self {
        let mut types = self.0.clone();
        let id = TypeId::of::<T>();
        if !types.contains(&id) {
            types.push(id);
        }
        Self::new(types)
    }

    pub fn without<T: Component>(&self) -> Self {
        let id = TypeId::of::<T>();
        let types: Vec<_> = self.0.iter().copied().filter(|t| *t != id).collect();
        Self(types)
    }

    pub fn contains<T: Component>(&self) -> bool {
        self.0.contains(&TypeId::of::<T>())
    }
}

/// One archetype: a set of densely-packed same-shape entities.
///
/// Every entity in this archetype has exactly the components described by
/// the key.  Components are stored column-major: one `Vec<T>` per type.
/// Row `i` corresponds to `entities[i]`.
pub struct Archetype {
    pub(crate) id: ArchetypeId,
    pub(crate) key: ArchetypeKey,
    /// Ordered parallel to `key.0` for fast column lookup by TypeId.
    pub(crate) columns: HashMap<TypeId, Box<dyn ErasedColumn>>,
    /// Entities occupying each row.
    pub(crate) entities: Vec<Entity>,
}

impl Archetype {
    pub(crate) fn new_empty(id: ArchetypeId) -> Self {
        Self {
            id,
            key: ArchetypeKey(vec![]),
            columns: HashMap::default(),
            entities: Vec::new(),
        }
    }

    pub(crate) fn new(id: ArchetypeId, key: ArchetypeKey) -> Self {
        Self {
            id,
            key,
            columns: HashMap::default(),
            entities: Vec::new(),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub(crate) fn register_column<T: Component>(&mut self) {
        self.columns
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Column::<T>::new()));
    }

    /// Typed column accessor — panics if this archetype doesn't hold `T`.
    #[inline]
    pub(crate) fn column<T: Component>(&self) -> &Column<T> {
        self.columns[&TypeId::of::<T>()]
            .as_any()
            .downcast_ref::<Column<T>>()
            .expect("column type mismatch — this is a bug in pulsar_game")
    }

    /// Typed mutable column accessor.
    #[inline]
    pub(crate) fn column_mut<T: Component>(&mut self) -> &mut Column<T> {
        self.columns
            .get_mut(&TypeId::of::<T>())
            .expect("column not registered")
            .as_any_mut()
            .downcast_mut::<Column<T>>()
            .expect("column type mismatch")
    }

    pub(crate) fn has_column<T: Component>(&self) -> bool {
        self.columns.contains_key(&TypeId::of::<T>())
    }

    /// Remove entity at `row` via swap-remove and drop all its components.
    /// Returns the entity that was swapped into `row` (if any), so the caller
    /// can update that entity's metadata.
    pub(crate) fn remove_row(&mut self, row: usize) -> Option<Entity> {
        let last = self.entities.len() - 1;
        for col in self.columns.values_mut() {
            // Safety: swap_remove returns owned ptr; we immediately drop it.
            let ptr = unsafe { col.swap_remove_erased(row) };
            unsafe { col.drop_erased(ptr) };
        }
        self.entities.swap_remove(row);
        if row < self.entities.len() {
            Some(self.entities[row])
        } else {
            None
        }
    }
}
