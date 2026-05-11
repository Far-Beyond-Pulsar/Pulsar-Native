use crate::archetype::Archetype;
use crate::component::{Column, Component};
use crate::entity::Entity;
use crate::world::World;
use std::any::TypeId;
use std::marker::PhantomData;

/// Trait implemented by tuples of component references, enabling typed queries.
///
/// `'w` is the lifetime of the world borrow.  The `Item` associated type is
/// the tuple you get per entity, e.g. `(&'w Position, &'w mut Velocity)`.
pub trait WorldQuery<'w>: Sized {
    type Item;

    /// Return `true` if `archetype` contains every component this query needs.
    fn matches(archetype: &Archetype) -> bool;

    /// Extract the item for row `row` from `archetype`.
    ///
    /// # Safety
    /// Caller must ensure that:
    /// - `archetype` matches this query (`matches` returned `true`).
    /// - `row` is within bounds.
    /// - No other conflicting borrows exist for the same row.
    unsafe fn fetch(archetype: &'w Archetype, row: usize) -> Self::Item;
}

// ── Single-component queries ─────────────────────────────────────────────────

impl<'w, T: Component> WorldQuery<'w> for &'w T {
    type Item = &'w T;

    fn matches(arch: &Archetype) -> bool {
        arch.columns.contains_key(&TypeId::of::<T>())
    }

    unsafe fn fetch(arch: &'w Archetype, row: usize) -> &'w T {
        arch.columns[&TypeId::of::<T>()]
            .as_any()
            .downcast_ref::<Column<T>>()
            .unwrap_unchecked()
            .data
            .get_unchecked(row)
    }
}

impl<'w, T: Component> WorldQuery<'w> for &'w mut T {
    type Item = &'w mut T;

    fn matches(arch: &Archetype) -> bool {
        arch.columns.contains_key(&TypeId::of::<T>())
    }

    unsafe fn fetch(arch: &'w Archetype, row: usize) -> &'w mut T {
        // Safety: caller guarantees exclusive access for this row.
        // We go through a raw pointer to bypass the shared borrow on `arch`.
        let col: &Column<T> = arch.columns[&TypeId::of::<T>()]
            .as_any()
            .downcast_ref::<Column<T>>()
            .unwrap_unchecked();
        let ptr = col.data.as_ptr() as *mut T;
        &mut *ptr.add(row)
    }
}

// ── Tuple queries (up to 8 components) ──────────────────────────────────────

macro_rules! impl_world_query_tuple {
    ($($Q:ident),+) => {
        impl<'w, $($Q: WorldQuery<'w>),+> WorldQuery<'w> for ($($Q,)+) {
            type Item = ($($Q::Item,)+);

            fn matches(arch: &Archetype) -> bool {
                $($Q::matches(arch))&&+
            }

            unsafe fn fetch(arch: &'w Archetype, row: usize) -> Self::Item {
                ($($Q::fetch(arch, row),)+)
            }
        }
    };
}

impl_world_query_tuple!(A);
impl_world_query_tuple!(A, B);
impl_world_query_tuple!(A, B, C);
impl_world_query_tuple!(A, B, C, D);
impl_world_query_tuple!(A, B, C, D, E);
impl_world_query_tuple!(A, B, C, D, E, F);
impl_world_query_tuple!(A, B, C, D, E, F, G);
impl_world_query_tuple!(A, B, C, D, E, F, G, H);

// ── QueryIter ────────────────────────────────────────────────────────────────

/// An iterator over all entities in a world that match query `Q`.
///
/// Yields `(Entity, Q::Item)` pairs.  The iterator holds a shared borrow of
/// the world for the duration of iteration, so no structural changes (spawn /
/// despawn / insert / remove) are possible while iterating.
pub struct QueryIter<'w, Q: WorldQuery<'w>> {
    archetypes: &'w [crate::archetype::Archetype],
    arch_idx: usize,
    row: usize,
    _marker: PhantomData<Q>,
}

impl<'w, Q: WorldQuery<'w>> QueryIter<'w, Q> {
    pub(crate) fn new(world: &'w World) -> Self {
        Self {
            archetypes: &world.archetypes,
            arch_idx: 0,
            row: 0,
            _marker: PhantomData,
        }
    }
}

impl<'w, Q: WorldQuery<'w>> Iterator for QueryIter<'w, Q> {
    type Item = (Entity, Q::Item);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let arch = self.archetypes.get(self.arch_idx)?;
            if !Q::matches(arch) {
                self.arch_idx += 1;
                self.row = 0;
                continue;
            }
            if self.row >= arch.entities.len() {
                self.arch_idx += 1;
                self.row = 0;
                continue;
            }
            let entity = arch.entities[self.row];
            // Safety: we hold a shared borrow of the world, arch matches Q,
            // and row is within bounds.
            let item = unsafe { Q::fetch(arch, self.row) };
            self.row += 1;
            return Some((entity, item));
        }
    }
}

// ── World query helpers ──────────────────────────────────────────────────────

impl World {
    /// Iterate over all entities that have every component referenced by `Q`.
    ///
    /// ```ignore
    /// for (entity, (pos, vel)) in world.query::<(&Position, &mut Velocity)>() {
    ///     vel.x += 1.0;
    /// }
    /// ```
    pub fn query<'w, Q: WorldQuery<'w>>(&'w self) -> QueryIter<'w, Q> {
        QueryIter::new(self)
    }
}
