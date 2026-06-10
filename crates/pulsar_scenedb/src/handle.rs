use std::fmt;

/// A packed 64-bit handle per SceneDB 2.0 spec §3 / CONTRACTS.md C1.
///
/// Bits 0–31: stable slot index. Bits 32–63: generation.
/// Generation 0 is permanently reserved as invalid; live generations start at 1.
/// Unlike row positions, the slot index is stable for the allocation lifetime —
/// the registry's slot→row table absorbs compaction movement.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Handle(u64);

impl Handle {
    /// The canonical invalid handle (all zero — generation 0).
    pub const INVALID: Handle = Handle(0);

    #[inline]
    #[must_use]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self(((generation as u64) << 32) | (index as u64))
    }

    /// Stable slot index (bits 0–31). NOT a row offset — resolve through the
    /// registry's slot→row table.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0 as u32
    }

    /// Generation (bits 32–63). 0 = invalid.
    #[inline]
    #[must_use]
    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    #[inline]
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.generation() != 0
    }

    /// The raw packed value (e.g. for GPU upload).
    #[inline]
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({}v{})", self.index(), self.generation())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_and_unpacks() {
        let h = Handle::new(14, 2);
        assert_eq!(h.index(), 14);
        assert_eq!(h.generation(), 2);
    }

    #[test]
    fn generation_zero_is_invalid() {
        assert!(!Handle::new(14, 0).is_valid());
        assert!(Handle::new(14, 1).is_valid());
        assert!(!Handle::INVALID.is_valid());
        assert_eq!(Handle::INVALID.index(), 0);
        assert_eq!(Handle::INVALID.generation(), 0);
    }

    #[test]
    fn max_index_and_generation_roundtrip() {
        let h = Handle::new(u32::MAX, u32::MAX);
        assert_eq!(h.index(), u32::MAX);
        assert_eq!(h.generation(), u32::MAX);
    }
}
