//! THE versioned VM TypeSlot encoding spec (#644) -- the contract Phase D's
//! `comp_*` opcodes and every `DispatchFn` glue consume when moving values
//! through the byte arena.
//!
//! ## Relationship to the existing blueprint ABI
//!
//! The blueprint VM (`pbgc`/`pulsar_std`) already passes
//! `pulsar_std::TypeSlot { size: usize, align: usize }` as `DispatchFn`'s
//! third argument, for GENERIC type parameters only. This module defines the
//! EXTENDED slot descriptor scripts use for TYPED component values:
//!
//! ```text
//! VmTypeSlot (repr(C), 24 bytes):
//!   offset 0   size     u64   -- RuntimeTypeInfo::size
//!   offset 8   align    u64   -- RuntimeTypeInfo::align
//!   offset 16  kind     u32   -- VmValueKind discriminant
//!   offset 20  reserved u32   -- MUST be 0; readers refuse otherwise
//! ```
//!
//! The first 16 bytes are byte-for-byte a `pulsar_std::TypeSlot`, so a
//! reader built for the old scheme can read the prefix safely. D wires
//! `pulsar_std::TypeSlot` construction through [`slot_for`] rather than
//! growing that struct in place (it is vendored-ABI surface).
//!
//! ## Value encoding per kind (encoding v1)
//!
//! How the bytes a slot POINTS AT are laid out. All integers are NATIVE
//! endian: this is an in-process calling convention between the executor
//! and dispatch functions sharing one arena, NOT a portable serialization
//! format (cross-process persistence goes through the JSON legs).
//!
//! - **[`VmValueKind::Direct`]** -- the value's own Rust memory bytes,
//!   inline at natural size/align. Zero transformation, zero allocation.
//!   Covers every registered Primitive-shaped type of ≤ 8 bytes (all
//!   numeric primitives, `bool` as one byte, and `Entity` as its packed
//!   `bits()` u64).
//! - **[`VmValueKind::Utf8String`]** -- `[u64 byte_len][utf8 payload]`.
//!   Allocation case: the payload lives in the arena's staged region.
//! - **[`VmValueKind::Vector`]** -- `[u64 count][count × element Direct
//!   bytes]`. Only vectors whose element classifies as Direct; elements sit
//!   back-to-back (callers keep natural alignment relative to payload
//!   start).
//! - **[`VmValueKind::JsonEncoded`]** -- `[u64 byte_len][utf8 JSON of the
//!   whole value]`, produced/consumed by the runtime type registry. The
//!   universal fallback: every REGISTERED reflected type encodes correctly
//!   here (nested structs, enums, wrappers -- including the `Vec<T>` /
//!   `Option<T>` registrations shimmed in [`crate::type_shims`]), so no
//!   supported property ever fails to marshal.
//!
//! A type not registered with reflection at all marshals under NO kind --
//! conversion fails with a typed error instead of guessing.
//!
//! ## Versioning rule
//!
//! [`TYPE_SLOT_ENCODING_VERSION`] bumps on ANY layout change above. Readers
//! must refuse descriptors whose `kind` discriminant or `reserved` field
//! they do not understand -- refuse, never guess.

/// Encoding version this workspace implements. Bump on any change to the
/// [`VmTypeSlot`] layout or the per-kind value layouts documented in this
/// module.
pub const TYPE_SLOT_ENCODING_VERSION: u32 = 1;

/// Extended runtime slot descriptor for typed component values crossing the
/// VM arena (see module doc for the full encoding spec and compatibility
/// rules). Construct via [`slot_for`], never by hand.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmTypeSlot {
    /// Natural size of the value ([`RuntimeTypeInfo::size`]).
    pub size: u64,
    /// Natural alignment of the value ([`RuntimeTypeInfo::align`]).
    pub align: u64,
    /// Which per-kind value layout follows this descriptor.
    pub kind: u32,
    /// Must be zero. Non-zero = descriptor from a future/foreign encoding;
    /// readers must refuse it.
    pub reserved: u32,
}

impl VmTypeSlot {
    /// Byte-for-byte the legacy `pulsar_std::TypeSlot` view of this
    /// descriptor's prefix (`(size, align)`) -- the bridge D's codegen
    /// emits for nodes still speaking the unextended ABI.
    pub fn legacy_prefix(&self) -> (u64, u64) {
        (self.size, self.align)
    }
}

/// Which per-kind value layout carries a slot's bytes (v1 discriminants are
/// wire-stable; never renumber).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmValueKind {
    /// Inline native bytes (primitives ≤ 8 bytes, `bool`, packed `Entity`).
    Direct = 0,
    /// `[u64 len][utf8]`.
    Utf8String = 1,
    /// `[u64 count][count × Direct elements]`.
    Vector = 2,
    /// `[u64 len][utf8 JSON]` -- universal registry-driven fallback.
    JsonEncoded = 3,
}

impl VmValueKind {
    /// Wire discriminant for [`VmTypeSlot::kind`].
    pub fn discriminant(self) -> u32 {
        self as u32
    }

    /// Inverse of [`Self::discriminant`]; `None` = unknown encoding from a
    /// newer producer -- callers must refuse, never guess.
    pub fn from_discriminant(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Direct),
            1 => Some(Self::Utf8String),
            2 => Some(Self::Vector),
            3 => Some(Self::JsonEncoded),
            _ => None,
        }
    }
}

use pulsar_reflection::{RuntimeTypeInfo, TypeStructure, WrapperType};

use crate::marshal;

/// Classify `type_info` into its v1 value kind -- the ONE decision both
/// encode and decode drive from, so a value always round-trips through the
/// same form. `Err` names types with no registration (nothing is known
/// about their representation).
pub fn classify(type_info: &'static RuntimeTypeInfo) -> Result<VmValueKind, String> {
    if marshal::is_direct_type(type_info.type_id) {
        return Ok(VmValueKind::Direct);
    }
    if type_info.type_id == std::any::TypeId::of::<String>() {
        return Ok(VmValueKind::Utf8String);
    }
    if let TypeStructure::Wrapper {
        wrapper_kind: WrapperType::Vec,
        inner,
    } = &type_info.structure
    {
        if marshal::is_direct_type(inner.type_id) {
            return Ok(VmValueKind::Vector);
        }
    }
    // Everything else REGISTERED rides the universal JSON fallback.
    if pulsar_reflection::RUNTIME_TYPE_REGISTRY.has_type_id(type_info.type_id) {
        return Ok(VmValueKind::JsonEncoded);
    }
    Err(format!(
        "type '{}' is not registered with reflection; no VM encoding exists",
        type_info.type_name
    ))
}

/// Build the arena slot descriptor for `type_info`: classification plus the
/// type's declared size/align. Refuses unregistered types (see
/// [`classify`]) rather than emitting a meaningless descriptor.
pub fn slot_for(type_info: &'static RuntimeTypeInfo) -> Result<VmTypeSlot, String> {
    Ok(VmTypeSlot {
        size: type_info.size as u64,
        align: type_info.align as u64,
        kind: classify(type_info)?.discriminant(),
        reserved: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::{RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY};
    use pulsar_scenedb::Entity;

    fn info_of<T: 'static>() -> &'static RuntimeTypeInfo {
        RUNTIME_TYPE_REGISTRY
            .get::<T>()
            .unwrap_or_else(|| panic!("{} should be registered", std::any::type_name::<T>()))
    }

    /// Hand-built descriptor for a type whose classification comes from the
    /// direct/fast-path set (Entity's registration lives in the
    /// object-model crate, not this binary): classification checks
    /// TypeIds directly, no registry entry required.
    static ENTITY_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
        type_id: std::any::TypeId::of::<Entity>(),
        type_name: "pulsar_scenedb::Entity",
        size: std::mem::size_of::<Entity>(),
        align: std::mem::align_of::<Entity>(),
        structure: TypeStructure::Primitive,
        color: None,
    };

    /// A real local type nothing registers anywhere -- classify must
    /// refuse its descriptor.
    struct NeverRegistered;

    static UNREGISTERED_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
        type_id: std::any::TypeId::of::<NeverRegistered>(),
        type_name: "tests::NeverRegistered",
        size: 16,
        align: 8,
        structure: TypeStructure::Primitive,
        color: None,
    };

    /// #644: the extended descriptor keeps `pulsar_std::TypeSlot`'s layout
    /// as its first 16 bytes ({size, align}), so legacy prefix readers stay
    /// valid; `kind` starts at offset 16, `reserved` at 20.
    #[test]
    fn vm_type_slot_is_a_layout_superset_of_the_legacy_slot() {
        assert_eq!(std::mem::offset_of!(VmTypeSlot, size), 0);
        assert_eq!(std::mem::offset_of!(VmTypeSlot, align), 8);
        assert_eq!(std::mem::offset_of!(VmTypeSlot, kind), 16);
        assert_eq!(std::mem::offset_of!(VmTypeSlot, reserved), 20);
        assert_eq!(std::mem::size_of::<VmTypeSlot>(), 24);

        let slot = slot_for(info_of::<f32>()).unwrap();
        assert_eq!(slot.legacy_prefix(), (4, 4));
    }

    /// #644: classification is deterministic and covers the corpus --
    /// direct primitives, packed Entity, strings, direct-element vectors,
    /// JSON fallback for everything else registered, refusal for anything
    /// unregistered.
    #[test]
    fn classification_covers_all_supported_kinds() {
        assert_eq!(classify(info_of::<f32>()).unwrap(), VmValueKind::Direct);
        assert_eq!(classify(info_of::<i32>()).unwrap(), VmValueKind::Direct);
        assert_eq!(classify(info_of::<bool>()).unwrap(), VmValueKind::Direct);
        assert_eq!(classify(&ENTITY_INFO).unwrap(), VmValueKind::Direct);
        assert_eq!(
            classify(info_of::<String>()).unwrap(),
            VmValueKind::Utf8String
        );

        assert_eq!(
            classify(info_of::<Vec<f32>>()).unwrap(),
            VmValueKind::Vector
        );
        assert_eq!(
            classify(info_of::<Vec<i32>>()).unwrap(),
            VmValueKind::Vector
        );

        // Registered non-direct/non-string/non-direct-vec: JSON fallback
        // (Vec<String>'s registration is shimmed in crate::type_shims).
        assert_eq!(
            classify(info_of::<Vec<String>>()).unwrap(),
            VmValueKind::JsonEncoded
        );

        assert!(
            classify(&UNREGISTERED_INFO).is_err(),
            "unregistered types are refused"
        );
    }

    /// #644: kind discriminants are wire-stable and total (no gaps; unknown
    /// values refuse).
    #[test]
    fn kind_discriminants_are_stable_and_total() {
        assert_eq!(VmValueKind::Direct.discriminant(), 0);
        assert_eq!(VmValueKind::Utf8String.discriminant(), 1);
        assert_eq!(VmValueKind::Vector.discriminant(), 2);
        assert_eq!(VmValueKind::JsonEncoded.discriminant(), 3);
        for raw in 0..=3u32 {
            assert_eq!(
                VmValueKind::from_discriminant(raw).unwrap().discriminant(),
                raw
            );
        }
        assert_eq!(VmValueKind::from_discriminant(4), None);
    }
}
