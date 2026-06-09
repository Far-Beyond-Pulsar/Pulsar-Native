use crate::archetype::Archetype;
use crate::component::{Column, Component};
use crate::entity::Entity;
use crate::world::World;
use std::any::TypeId;
use std::marker::PhantomData;

pub trait WorldQuery<'w>: Sized {
    type Item;

    fn matches(archetype: &Archetype) -> bool;

    unsafe fn fetch(archetype: &'w Archetype, row: usize) -> Self::Item;
}

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
        let col: &Column<T> = arch.columns[&TypeId::of::<T>()]
            .as_any()
            .downcast_ref::<Column<T>>()
            .unwrap_unchecked();
        let ptr = col.data.as_ptr() as *mut T;
        &mut *ptr.add(row)
    }
}

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
            let item = unsafe { Q::fetch(arch, self.row) };
            self.row += 1;
            return Some((entity, item));
        }
    }
}

impl World {
    pub fn query<'w, Q: WorldQuery<'w>>(&'w self) -> QueryIter<'w, Q> {
        QueryIter::new(self)
    }
}
