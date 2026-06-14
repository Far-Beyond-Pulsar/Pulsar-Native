use std::fmt;

/// A lightweight handle to an entity in the ECS [`World`](crate::World).
///
/// Internally a packed `u64`: the lower 32 bits are the entity index (slot in
/// [`World::entity_slots`]) and the upper 32 bits are the generation counter.
/// The generation is incremented each time the slot is recycled, which lets
/// [`World::is_alive`] reject stale handles.
///
/// `Entity` is `Copy`, cheap to pass around, and `DANGLING` can be used as
/// a sentinel value.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Entity(u64);

impl Entity {
    #[inline]
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        Self(((generation as u64) << 32) | (index as u64))
    }

    /// The slot index within [`World::entity_slots`](crate::World).
    #[inline]
    pub fn index(self) -> u32 {
        self.0 as u32
    }

    /// The generation counter for stale-handle detection.
    ///
    /// Each time a slot is recycled the generation is incremented.  An entity
    /// handle is alive iff `handle.generation() == world.entity_slots[handle.index()].generation`.
    #[inline]
    pub fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Sentinel value for a dead or null entity.
    ///
    /// `u64::MAX` â€” guaranteed not to collide with any valid entity because
    /// entity indices are bounded by the slot-vec length.  Useful as an
    /// initialiser for option-like patterns without heap allocation.
    pub const DANGLING: Entity = Entity(u64::MAX);
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entity({}v{})", self.index(), self.generation())
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EntitySlot {
    pub generation: u32,
    pub archetype: crate::archetype::ArchetypeId,
    pub row: u32,
}

impl EntitySlot {
    pub(crate) fn empty(generation: u32) -> Self {
        Self {
            generation,
            archetype: crate::archetype::ArchetypeId::EMPTY,
            row: 0,
        }
    }
}
