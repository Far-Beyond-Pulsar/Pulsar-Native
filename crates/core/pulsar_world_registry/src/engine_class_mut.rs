//! The write guard behind `WorldComponentRegistration::get_as_engine_class_mut`
//! (#841).
//!
//! A bare `&mut dyn EngineClass` taken from `Mut::into_inner` reports the
//! write *when it is borrowed*: every SceneDB hook (GPU mirror upload, change
//! tracker, subscriptions, journals) fires with the old value, and the edit
//! that follows is never observed. [`EngineClassMut`] keeps the SceneDB guard
//! alive instead: it derefs to `dyn EngineClass`, and the hooks fire when it
//! is dropped, after the edit, like `Mut` and `MutDyn`. A guard that was only
//! read through (`Deref`) reports nothing.

use std::any::Any;
use std::ops::{Deref, DerefMut};

use pulsar_reflection::EngineClass;
use pulsar_scenedb::{component_id, Entity, MutDyn, World};

/// A live, mutable borrow of a registered component as `dyn EngineClass`.
/// Dropping it reports the write to SceneDB if (and only if) it was written
/// through.
pub struct EngineClassMut<'w> {
    guard: MutDyn<'w>,
    as_ref: fn(&dyn Any) -> &dyn EngineClass,
    as_mut: fn(&mut dyn Any) -> &mut dyn EngineClass,
}

impl<'w> EngineClassMut<'w> {
    /// Borrow `entity`'s `T` as a write guard; `None` if it has none.
    pub fn of<T: EngineClass + Any>(world: &'w mut World, entity: Entity) -> Option<Self> {
        let guard = world.get_dyn_mut(entity, component_id::<T>())?;
        Some(Self { guard, as_ref: upcast_ref::<T>, as_mut: upcast_mut::<T> })
    }
}

fn upcast_ref<T: EngineClass + Any>(value: &dyn Any) -> &dyn EngineClass {
    value.downcast_ref::<T>().expect("EngineClassMut guards the component type it was built for")
}

fn upcast_mut<T: EngineClass + Any>(value: &mut dyn Any) -> &mut dyn EngineClass {
    value.downcast_mut::<T>().expect("EngineClassMut guards the component type it was built for")
}

impl Deref for EngineClassMut<'_> {
    type Target = dyn EngineClass;
    fn deref(&self) -> &Self::Target {
        // `MutDyn`'s `Deref` is the read-only view: no mutation is recorded.
        (self.as_ref)(&*self.guard)
    }
}

impl DerefMut for EngineClassMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // `MutDyn`'s `DerefMut` marks the guard mutated; its hooks run on drop.
        (self.as_mut)(&mut *self.guard)
    }
}
