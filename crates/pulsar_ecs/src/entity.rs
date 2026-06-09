use std::fmt;

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Entity(u64);

impl Entity {
    #[inline]
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        Self(((generation as u64) << 32) | (index as u64))
    }

    #[inline]
    pub fn index(self) -> u32 {
        self.0 as u32
    }

    #[inline]
    pub fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

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
