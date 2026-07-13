use crate::component::{component_id, ComponentId};
use crate::archetype::Archetype;
use crate::component::{Column, Component};
use crate::entity::Entity;
use crate::world::World;
use std::marker::PhantomData;

/// Types that can be fetched from an archetype row during a query.
///
/// Implementations exist for:
/// - `&T` â€” shared reference to a component
/// - `&mut T` â€” mutable reference to a component
/// - `()` â€” matches every archetype (for counting or iteration without data)
/// - Tuples `(A, B, ...)` up to 8 elements â€” combine multiple fetches
///
/// # Safety
///
/// `fetch` uses `unsafe` because it performs unchecked column access.  The
/// caller guarantees that `matches(archetype)` is `true` and `row` is within
/// the archetype's entity count.
pub trait WorldQuery<'w>: Sized {
    /// The type returned by [`fetch`](Self::fetch).
    type Item;

    /// Returns `true` if the given archetype contains all the components
    /// required by this query.
    fn matches(archetype: &Archetype) -> bool;

    /// Read component data at `row` in `archetype`.
    ///
    /// # Safety
    ///
    /// - `archetype` must satisfy `Self::matches(archetype)`.
    /// - `row` must be < `archetype.entities.len()`.
    unsafe fn fetch(archetype: &'w Archetype, row: usize) -> Self::Item;
}

impl<'w, T: Component> WorldQuery<'w> for &'w T {
    type Item = &'w T;

    #[inline]
    fn matches(arch: &Archetype) -> bool {
        let cid = component_id::<T>();
        arch.has_columns(std::slice::from_ref(&cid))
    }

    // SAFETY: caller guarantees archetype matches and row is in bounds.
    #[inline]
    unsafe fn fetch(arch: &'w Archetype, row: usize) -> &'w T {
        let cid = component_id::<T>();
        // SAFETY: caller guarantees the column exists and row is in bounds.
        let col = arch.columns.get_unchecked(cid.0 as usize)
            .as_ref()
            .unwrap_unchecked()
            .as_any()
            .downcast_ref::<Column<T>>()
            .unwrap_unchecked();
        &col.data.get_unchecked(row)
    }
}

impl<'w, T: Component> WorldQuery<'w> for &'w mut T {
    type Item = &'w mut T;

    #[inline]
    fn matches(arch: &Archetype) -> bool {
        let cid = component_id::<T>();
        arch.has_columns(std::slice::from_ref(&cid))
    }

    // SAFETY: caller guarantees archetype matches and row is in bounds.
    #[inline]
    unsafe fn fetch(arch: &'w Archetype, row: usize) -> &'w mut T {
        let cid = component_id::<T>();
        // SAFETY: caller guarantees the column exists and row is in bounds.
        let col: &Column<T> = arch.columns.get_unchecked(cid.0 as usize)
            .as_ref()
            .unwrap_unchecked()
            .as_any()
            .downcast_ref::<Column<T>>()
            .unwrap_unchecked();
        let ptr = col.data.as_ptr() as *mut T;
        &mut *ptr.add(row)
    }
}

// â”€â”€ Empty query: matches every archetype (useful for counting entities) â”€â”€â”€â”€â”€â”€

impl<'w> WorldQuery<'w> for () {
    type Item = ();
    #[inline]
    fn matches(_arch: &Archetype) -> bool {
        true
    }
    // SAFETY: caller guarantees row is in bounds.
    #[inline]
    unsafe fn fetch(_arch: &'w Archetype, _row: usize) -> Self::Item {}
}

// â”€â”€ Tuple conbinator macro (1 to 8 components) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

macro_rules! impl_world_query_tuple {
    ($($Q:ident),+) => {
        impl<'w, $($Q: WorldQuery<'w>),+> WorldQuery<'w> for ($($Q,)+) {
            type Item = ($($Q::Item,)+);

            #[inline]
            fn matches(arch: &Archetype) -> bool {
                $($Q::matches(arch))&&+
            }

            // SAFETY: caller guarantees all Q::matches & row in bounds.
            #[inline]
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

// â”€â”€ QueryIter â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Iterator over all entities in the [`World`](crate::World) that match a query
/// pattern `Q`.
///
/// Yields `(Entity, Q::Item)` pairs.  Created by [`World::query`](crate::World::query).
///
/// The iterator scans archetypes in order, skipping those that don't match `Q`.
/// Within each matching archetype it walks rows sequentially.
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
            // SAFETY: we've verified that this archetype matches Q and
            // that self.row is in bounds.
            let item = unsafe { Q::fetch(arch, self.row) };
            self.row += 1;
            return Some((entity, item));
        }
    }
}

impl World {
    /// Iterate all entities whose components match the query pattern `Q`.
    ///
    /// # Example
    ///
    /// ```
    /// use pulsar_scenedb::{World, QueryIter, WorldQuery};
    ///
    /// # struct Pos(f32, f32);
    /// # struct Vel(f32, f32);
    /// # let mut world = World::new();
    /// for (pos, vel) in world.query::<(&Pos, &Vel)>() {
    ///     // ...
    /// }
    /// ```
    ///
    /// An empty tuple `()` matches every archetype and can be used to iterate
    /// all entities without fetching any component data.
    pub fn query<'w, Q: WorldQuery<'w>>(&'w self) -> QueryIter<'w, Q> {
        QueryIter::new(self)
    }
}