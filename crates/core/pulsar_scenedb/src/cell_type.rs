use crate::component::ComponentId;
use crate::page::{ColumnDesc, GenericColumnAny, LayoutError, MAX_STRIDE_BYTES};
use crate::token::TypeToken;
use std::any::TypeId;

#[derive(Debug, PartialEq, Eq)]
pub enum CellTypeError {
    /// Combined stride (slot-ID column + all user columns) exceeds 128 bytes.
    StrideExceeded { stride: u32 },
    /// The same token was declared twice.
    DuplicateColumn { id: ComponentId },
    /// No columns declared.
    Empty,
}

/// Descriptor for a single generic (non-Pod) column in a cell type.
/// Stores a constructor function pointer so `CellStorage::from_cell_type`
/// can produce the correct monomorphized `GenericColumn<T>` at runtime.
#[derive(Clone)]
pub(crate) struct GenericColumnDesc {
    pub component_id: ComponentId,
    pub type_id: TypeId,
    pub construct: fn(u32) -> Box<dyn GenericColumnAny>,
}

/// A registered cell composition: the ordered set of column types a cell
/// stores, validated holistically against the per-element stride budget
/// (§7.1 / CONTRACTS.md C2). Build with the fluent API, then hand to
/// [`CellStorage::from_cell_type`](crate::cell::CellStorage::from_cell_type).
#[derive(Clone)]
pub struct CellType {
    name: &'static str,
    tokens: Vec<TypeToken>,
    generic_entries: Vec<GenericColumnDesc>,
}

// Manual Debug: GenericColumnDesc's construct closure isn't Debug.
impl std::fmt::Debug for CellType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CellType")
            .field("name", &self.name)
            .field("tokens", &self.tokens)
            .field("generic_entries", &self.generic_entries.len())
            .finish()
    }
}

impl CellType {
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        Self { name, tokens: Vec::new(), generic_entries: Vec::new() }
    }

    /// Declare the next user Pod column. Order is significant: it becomes the
    /// user-column index.
    #[must_use]
    pub fn with(mut self, token: TypeToken) -> Self {
        self.tokens.push(token);
        self
    }

    /// Declare the next user generic (non-Pod) column. Order among generic
    /// columns is significant — it determines the generic-column index in
    /// `CellStorage::column_for_generic::<T>()`.
    #[must_use]
    pub fn with_generic<T: 'static>(mut self) -> Self {
        fn make<T_: 'static>(cap: u32) -> Box<dyn GenericColumnAny> {
            Box::new(crate::page::GenericColumn::<T_>::new(cap))
        }
        self.generic_entries.push(GenericColumnDesc {
            component_id: crate::component::component_id::<T>(),
            type_id: TypeId::of::<T>(),
            construct: make::<T>,
        });
        self
    }

    /// Validate and freeze the layout. Performs the **holistic** stride check
    /// across the implicit u32 slot-ID column plus every declared Pod user
    /// column (§7.1): splitting a layout into many small columns cannot bypass
    /// the 128-byte budget. Generic columns are not subject to the stride
    /// budget (they are stored outside the SoA allocation).
    pub fn build(self) -> Result<RegisteredCellType, CellTypeError> {
        if self.tokens.is_empty() && self.generic_entries.is_empty() {
            return Err(CellTypeError::Empty);
        }
        // Reject duplicate Pod column types (same dense id declared twice).
        let mut seen = std::collections::HashSet::with_capacity(self.tokens.len());
        for token in &self.tokens {
            if !seen.insert(token.id()) {
                return Err(CellTypeError::DuplicateColumn { id: token.id() });
            }
        }
        // Also check generic entries don't clash with Pod entries.
        for gen in &self.generic_entries {
            if !seen.insert(gen.component_id) {
                return Err(CellTypeError::DuplicateColumn { id: gen.component_id });
            }
        }
        // Holistic stride: slot-ID column (u32 = 4 bytes) + all Pod user columns.
        let user_stride: u32 = self.tokens.iter().map(|t| t.desc().size).sum();
        let stride = user_stride + ColumnDesc::of::<u32>().size;
        if stride > MAX_STRIDE_BYTES {
            return Err(CellTypeError::StrideExceeded { stride });
        }
        Ok(RegisteredCellType {
            name: self.name,
            tokens: self.tokens,
            generic_entries: self.generic_entries,
        })
    }
}

/// A validated cell composition. Maps tokens → user-column indices and yields
/// the `ColumnDesc` list a `CellStorage` page needs.
#[derive(Clone)]
pub struct RegisteredCellType {
    name: &'static str,
    pub(crate) tokens: Vec<TypeToken>,
    pub(crate) generic_entries: Vec<GenericColumnDesc>,
}

impl std::fmt::Debug for RegisteredCellType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredCellType")
            .field("name", &self.name)
            .field("tokens", &self.tokens)
            .field("generic_entries", &self.generic_entries.len())
            .finish()
    }
}

/// Trait for types that know their SceneDB column layout.
/// Generated by `#[derive(SceneStore)]`.
pub trait SceneColumnSet: crate::page::Pod + crate::token::HasTypeToken {
    /// Build the `RegisteredCellType` for this component's field layout.
    fn cell_type() -> RegisteredCellType;

    /// Create a `CellStorage` backed by this type's column layout.
    fn create_cell(capacity: u32) -> Result<crate::cell::CellStorage, LayoutError> {
        crate::cell::CellStorage::from_cell_type(&Self::cell_type(), capacity)
    }
}

impl RegisteredCellType {
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub fn user_column_count(&self) -> usize {
        self.tokens.len()
    }

    /// User-column index for a token, or None if the token isn't part of this
    /// cell type.
    #[must_use]
    pub fn column_index(&self, token: TypeToken) -> Option<usize> {
        self.tokens.iter().position(|t| t.id() == token.id())
    }

    /// The Pod user-column `ColumnDesc` list (in declaration order) for building
    /// the page layout.
    #[must_use]
    pub fn user_descs(&self) -> Vec<ColumnDesc> {
        self.tokens.iter().map(|t| t.desc()).collect()
    }

    /// Token dense ids in declaration order (for building token→index maps).
    #[must_use]
    pub fn token_ids(&self) -> Vec<ComponentId> {
        self.tokens.iter().map(|t| t.id()).collect()
    }

    /// Generic entry descriptor data (component id + type id) in declaration
    /// order.
    pub(crate) fn generic_descs(&self) -> Vec<(ComponentId, TypeId)> {
        self.generic_entries
            .iter()
            .map(|e| (e.component_id, e.type_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TypeToken;

    #[test]
    fn builds_layout_from_tokens() {
        let ct = CellType::new("test")
            .with(TypeToken::of::<f32>())
            .with(TypeToken::of::<u32>())
            .build()
            .unwrap();
        assert_eq!(ct.user_column_count(), 2);
        // Token resolves to its user-column index in declaration order.
        assert_eq!(ct.column_index(TypeToken::of::<f32>()), Some(0));
        assert_eq!(ct.column_index(TypeToken::of::<u32>()), Some(1));
        assert_eq!(ct.column_index(TypeToken::of::<u64>()), None);
    }

    // Four DISTINCT 32-byte Pod column types for the stride test (using the
    // same token twice would trip the duplicate check before the stride check).
    // These exist only as distinct column types for the stride fixture; their
    // field is never read (TypeToken::of keys on the type, not the value).
    #[allow(dead_code)] #[derive(Copy, Clone)] struct B32([u8; 32]);
    #[allow(dead_code)] #[derive(Copy, Clone)] struct C32([u8; 32]);
    #[allow(dead_code)] #[derive(Copy, Clone)] struct D32([u8; 32]);
    #[allow(dead_code)] #[derive(Copy, Clone)] struct E32([u8; 32]);
    // SAFETY: all-zero is valid for a byte array; Copy, no Drop.
    unsafe impl crate::page::Pod for B32 {}
    unsafe impl crate::page::Pod for C32 {}
    unsafe impl crate::page::Pod for D32 {}
    unsafe impl crate::page::Pod for E32 {}

    #[test]
    fn holistic_stride_check_rejects_over_budget() {
        // 4 distinct × 32 bytes = 128 user bytes + the 4-byte slot column → 132.
        let r = CellType::new("fat")
            .with(TypeToken::of::<B32>())
            .with(TypeToken::of::<C32>())
            .with(TypeToken::of::<D32>())
            .with(TypeToken::of::<E32>())
            .build();
        // Holistic budget counts the slot-ID column too: 128 + 4 = 132 > 128.
        assert!(matches!(r, Err(CellTypeError::StrideExceeded { stride: 132 })));
    }

    #[test]
    fn duplicate_token_rejected() {
        let r = CellType::new("dup")
            .with(TypeToken::of::<f32>())
            .with(TypeToken::of::<f32>())
            .build();
        assert!(matches!(r, Err(CellTypeError::DuplicateColumn { .. })));
    }
}
