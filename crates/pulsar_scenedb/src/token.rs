use crate::component::{component_id, ComponentId};
use crate::page::{ColumnDesc, Pod};
use pulsar_reflection::{RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY};
use std::any::TypeId;
use std::hash::{Hash, Hasher};

/// A dense, typed handle to a registered SceneDB column type (spec §7,
/// CONTRACTS.md C7).
///
/// Binds three things for a column element type `T: Pod`:
/// - a **dense `ComponentId`** (the crate's existing sequential u32 id-space,
///   reused so SceneDB columns and ECS components share one id allocator);
/// - the **`ColumnDesc`** (size/align) used to lay the column out in a page;
/// - the **`TypeId`**, used to look up the optional `pulsar_reflection`
///   `RuntimeTypeInfo` for serialization / editor metadata.
///
/// A `TypeToken` is `Copy` and cheap; construct it with [`TypeToken::of`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TypeToken {
    id: ComponentId,
    type_id: TypeId,
    desc: ColumnDesc,
}

// `ColumnDesc` does not implement `Hash`, so `Hash` is implemented by hand.
impl Hash for TypeToken {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // `id` is the dense canonical key (an injective function of the type);
        // hashing it alone is sufficient and consistent with the derived `Eq`.
        self.id.hash(state);
    }
}

impl TypeToken {
    /// Token for column element type `T`. Allocates `T`'s dense id on first
    /// use (per-process), then returns the same id forever.
    #[must_use]
    pub fn of<T: Pod + 'static>() -> Self {
        Self {
            id: component_id::<T>(),
            type_id: TypeId::of::<T>(),
            desc: ColumnDesc::of::<T>(),
        }
    }

    /// The dense column-type id (== `component_id::<T>()`).
    #[inline]
    #[must_use]
    pub fn id(self) -> ComponentId {
        self.id
    }

    /// The column layout descriptor for one element.
    #[inline]
    #[must_use]
    pub fn desc(self) -> ColumnDesc {
        self.desc
    }

    /// The `pulsar_reflection` metadata for this type, if it was registered
    /// (via `#[derive(Reflectable)]` / `#[pulsar_type]`). `None` for bare Pod
    /// types with no reflection registration — those still work as columns;
    /// they just carry no serialization/editor metadata.
    #[inline]
    #[must_use]
    pub fn type_info(self) -> Option<&'static RuntimeTypeInfo> {
        RUNTIME_TYPE_REGISTRY.get_by_id(self.type_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_carries_dense_id_and_layout() {
        let t = TypeToken::of::<f32>();
        assert_eq!(t.desc(), crate::page::ColumnDesc::of::<f32>());
        // Same type → same dense id (stable across calls).
        assert_eq!(TypeToken::of::<f32>().id(), t.id());
        // Different types → different ids.
        assert_ne!(TypeToken::of::<u32>().id(), t.id());
    }

    #[test]
    fn token_id_matches_component_id() {
        // The token id-space IS the crate's ComponentId allocator (C7).
        assert_eq!(TypeToken::of::<u64>().id(), crate::component::component_id::<u64>());
    }

    #[test]
    fn unregistered_type_has_no_reflection_info() {
        // A bare Pod type not registered with pulsar_reflection resolves to None.
        struct LocalUnregistered(#[allow(dead_code)] u32);
        // SAFETY: trivially Pod for the test (Copy + zero-valid). Not exported.
        unsafe impl crate::page::Pod for LocalUnregistered {}
        impl Clone for LocalUnregistered { fn clone(&self) -> Self { Self(self.0) } }
        impl Copy for LocalUnregistered {}
        assert!(TypeToken::of::<LocalUnregistered>().type_info().is_none());
    }
}
