use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::any::Any;
use std::mem::MaybeUninit;

/// Hard ceiling on per-page element capacity (spec §4.3).
pub const MAX_PAGE_CAPACITY: u32 = 1024;
/// Recommended default capacity (spec §4.3).
pub const DEFAULT_PAGE_CAPACITY: u32 = 256;
/// Combined per-element stride limit across all columns (spec §7.1, C2).
pub const MAX_STRIDE_BYTES: u32 = 128;
/// Every column starts on a cache-line boundary (spec §4.2).
pub const COLUMN_ALIGN: usize = 64;

/// Marker for types whose every byte pattern — in particular all-zero — is a
/// valid value, so a column of them may be handed out as `&[T]` over the
/// zero-initialised page allocation.
///
/// `unsafe` to implement: implementors guarantee zero-init validity and no
/// `Drop` glue. The M1b TypeToken layer builds the column-registration API on
/// top of this bound.
///
/// # Safety
/// All-zero bytes must be a valid value of `Self`, and `Self` must be `Copy`
/// with no `Drop`.
pub unsafe trait Pod: Copy {}

macro_rules! impl_pod {
    ($($t:ty),*) => { $( unsafe impl Pod for $t {} )* };
}
impl_pod!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64);
// C5 instance element: 64-byte row-major mat4. Kept in the graphics-free core
// so the transform column exists independent of the gpu feature.
unsafe impl Pod for [f32; 16] {}

// ── Non-Pod column support (pre-work item 1) ─────────────────────────────

/// Type-erased operations on a generic (non-Pod) column, so that `Page` and
/// `CellStorage` can swap / drop elements without knowing the concrete type.
pub(crate) trait GenericColumnAny: Send + Sync {
    fn push_row(&mut self);
    fn pop_row(&mut self);
    fn swap(&mut self, a: u32, b: u32);
    fn len(&self) -> usize;
    fn as_any_ref(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// A column of non-Pod elements backed by `MaybeUninit<T>`.  Each slot is
/// either initialized (a valid `T`) or uninitialized; an initialization
/// bitmap tracks which is which.
pub struct GenericColumn<T: 'static> {
    data: Vec<MaybeUninit<T>>,
    init_bits: Vec<u64>,
}

// SAFETY: `MaybeUninit` interior provides no extra thread-safety affordances;
// external `&`/`&mut` borrowing discipline is sufficient.
unsafe impl<T: 'static> Send for GenericColumn<T> {}
unsafe impl<T: 'static> Sync for GenericColumn<T> {}

impl<T: 'static> GenericColumn<T> {
    pub fn new(capacity: u32) -> Self {
        let cap = capacity as usize;
        Self {
            data: Vec::with_capacity(cap),
            init_bits: vec![0u64; cap.div_ceil(64)],
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn get(&self, idx: usize) -> Option<&T> {
        if idx < self.data.len() && self.is_init(idx) {
            Some(unsafe { self.data[idx].assume_init_ref() })
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, idx: usize) -> Option<&mut T> {
        if idx < self.data.len() && self.is_init(idx) {
            Some(unsafe { self.data[idx].assume_init_mut() })
        } else {
            None
        }
    }

    /// Write a value into slot `idx`.  Drops any previously-initialized value.
    pub fn set(&mut self, idx: usize, value: T) {
        if idx < self.data.len() && self.is_init(idx) {
            unsafe { self.data[idx].assume_init_drop(); }
            self.data[idx] = MaybeUninit::new(value);
        } else {
            // Extend vec if necessary (shouldn't happen in normal use — rows
            // are pushed before being written).
            while self.data.len() <= idx {
                self.data.push(MaybeUninit::uninit());
            }
            self.data[idx] = MaybeUninit::new(value);
        }
        self.set_init(idx);
    }

    /// Drop the value at `idx` and mark the slot uninitialized.
    pub fn free(&mut self, idx: usize) {
        if idx < self.data.len() && self.is_init(idx) {
            unsafe { self.data[idx].assume_init_drop(); }
            self.data[idx] = MaybeUninit::uninit();
            self.clear_init(idx);
        }
    }

    /// Swap elements and initialization bits at indices `a` and `b`.
    pub fn swap(&mut self, a: u32, b: u32) {
        let (a, b) = (a as usize, b as usize);
        // SAFETY: index bounds are caller's responsibility.
        unsafe {
            std::ptr::swap(
                self.data[a].as_ptr() as *mut MaybeUninit<T>,
                self.data[b].as_ptr() as *mut MaybeUninit<T>,
            );
        }
        // Swap init bits to keep them in sync with data (§4.5: every swap
        // must preserve the init-bit→row invariant; the previous omission
        // was GAP-1 / the init-bit desync soundness hole).
        let init_a = self.is_init(a);
        let init_b = self.is_init(b);
        if init_a != init_b {
            self.init_bits[a / 64] ^= 1u64 << (a % 64);
            self.init_bits[b / 64] ^= 1u64 << (b % 64);
        }
    }

    fn is_init(&self, idx: usize) -> bool {
        self.init_bits[idx / 64] & (1u64 << (idx % 64)) != 0
    }

    fn set_init(&mut self, idx: usize) {
        self.init_bits[idx / 64] |= 1u64 << (idx % 64);
    }

    fn clear_init(&mut self, idx: usize) {
        self.init_bits[idx / 64] &= !(1u64 << (idx % 64));
    }
}

impl<T: 'static> Drop for GenericColumn<T> {
    fn drop(&mut self) {
        for idx in 0..self.data.len() {
            if self.is_init(idx) {
                unsafe { self.data[idx].assume_init_drop(); }
            }
        }
    }
}

impl<T: 'static> GenericColumnAny for GenericColumn<T> {
    fn push_row(&mut self) {
        self.data.push(MaybeUninit::uninit());
    }

    fn pop_row(&mut self) {
        let idx = self.data.len().wrapping_sub(1);
        if self.is_init(idx) {
            unsafe { self.data[idx].assume_init_drop(); }
            self.clear_init(idx);
        }
        self.data.pop();
    }

    fn swap(&mut self, a: u32, b: u32) {
        GenericColumn::swap(self, a, b);
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A view over a Pod column's zero-initialized page memory.  Does not own
/// the data; the backing `Page` must outlive this view.
pub struct PodColumn<T: Pod> {
    pub(crate) data: *const T,
    pub(crate) len: usize,
    pub(crate) capacity: usize,
}

impl<T: Pod> PodColumn<T> {
    pub fn as_slice(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.data, self.len) }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// A typed column that is either backed by zero-initialized page memory
/// (Pod) or by `MaybeUninit<T>` heap storage (generic).
pub enum Column<T: Pod + 'static> {
    Pod(PodColumn<T>),
    Generic(GenericColumn<T>),
}

// ── End non-Pod column support ──────────────────────────────────────────

/// Size/alignment descriptor for one column's element type.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColumnDesc {
    pub size: u32,
    pub align: u32,
}

impl ColumnDesc {
    pub const fn of<T>() -> Self {
        Self {
            size: std::mem::size_of::<T>() as u32,
            align: std::mem::align_of::<T>() as u32,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LayoutError {
    StrideExceeded { stride: u32 },
    BadCapacity { capacity: u32 },
    AlignmentExceeded { align: u32 },
}

/// Computed byte layout for a page: per-column offsets within one contiguous
/// allocation, every column 64-byte aligned (spec §4.2 page header contract).
#[derive(Clone, Debug)]
pub struct PageLayout {
    column_descs: Vec<ColumnDesc>,
    column_offsets: Vec<usize>,
    capacity: u32,
    total_bytes: usize,
}

impl PageLayout {
    pub fn new(columns: &[ColumnDesc], capacity: u32) -> Result<Self, LayoutError> {
        if capacity == 0 || capacity > MAX_PAGE_CAPACITY {
            return Err(LayoutError::BadCapacity { capacity });
        }
        let stride: u32 = columns.iter().map(|c| c.size).sum();
        if stride > MAX_STRIDE_BYTES {
            return Err(LayoutError::StrideExceeded { stride });
        }
        let mut offsets = Vec::with_capacity(columns.len());
        let mut cursor = 0usize;
        for col in columns {
            if col.align as usize > COLUMN_ALIGN {
                return Err(LayoutError::AlignmentExceeded { align: col.align });
            }
            cursor = next_multiple(cursor, COLUMN_ALIGN);
            offsets.push(cursor);
            cursor += col.size as usize * capacity as usize;
        }
        Ok(Self {
            column_descs: columns.to_vec(),
            column_offsets: offsets,
            capacity,
            total_bytes: next_multiple(cursor, COLUMN_ALIGN),
        })
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    pub fn column_count(&self) -> usize {
        self.column_descs.len()
    }

    pub fn column_descs(&self) -> &[ColumnDesc] {
        &self.column_descs
    }
}

#[inline]
fn next_multiple(n: usize, m: usize) -> usize {
    // m is always COLUMN_ALIGN (64); n is bounded by MAX_STRIDE_BYTES * MAX_PAGE_CAPACITY.
    n.div_ceil(m).checked_mul(m).expect("page layout size overflow")
}

/// One SoA page: a single 64-byte-aligned contiguous allocation holding all
/// Pod columns, plus a parallel `Vec` of type-erased [`GenericColumnAny`]
/// boxes for non-Pod columns.  `len` counts live+dead rows up to the
/// compaction frontier; the liveness bitmask (liveness.rs) tracks which
/// are alive.  Generic columns grow/shrink in lockstep with `push_row` /
/// `pop_row`.
pub struct Page {
    data: *mut u8,
    layout: PageLayout,
    alloc_layout: Layout,
    len: u32,
    generic_columns: Vec<Box<dyn GenericColumnAny>>,
}

// SAFETY: Page owns its allocation exclusively; all access goes through
// &self/&mut self, so aliasing follows Rust's borrow rules.
unsafe impl Send for Page {}
unsafe impl Sync for Page {}

impl Page {
    pub fn new(layout: &PageLayout) -> Self {
        let alloc_layout =
            Layout::from_size_align(layout.total_bytes.max(COLUMN_ALIGN), COLUMN_ALIGN)
                .expect("page layout is valid");
        // SAFETY: size is non-zero (max'd with COLUMN_ALIGN), align is 64.
        let data = unsafe { alloc_zeroed(alloc_layout) };
        if data.is_null() {
            std::alloc::handle_alloc_error(alloc_layout);
        }
        Self {
            data,
            layout: layout.clone(),
            alloc_layout,
            len: 0,
            generic_columns: Vec::new(),
        }
    }

    #[inline]
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Returns `true` only if no rows have ever been pushed (`len == 0`).
    /// A page may have `len > 0` with every row dead — consult the liveness
    /// mask for true emptiness.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn capacity(&self) -> u32 {
        self.layout.capacity
    }

    pub fn layout(&self) -> &PageLayout {
        &self.layout
    }

    /// Reserve the next row, returning its index. None when full.
    /// Also grows every generic column by one uninitialized slot.
    pub fn push_row(&mut self) -> Option<u32> {
        if self.len >= self.layout.capacity {
            return None;
        }
        let row = self.len;
        self.len += 1;
        for gc in &mut self.generic_columns {
            gc.push_row();
        }
        Some(row)
    }

    /// Drop the last row (used by swap-and-pop compaction).
    /// Also drops the trailing element of every generic column.
    pub fn pop_row(&mut self) {
        debug_assert!(self.len > 0);
        self.len -= 1;
        for gc in &mut self.generic_columns {
            gc.pop_row();
        }
    }

    /// Number of generic columns stored on this page.
    pub fn generic_column_count(&self) -> usize {
        self.generic_columns.len()
    }

    /// Access a generic column by index (type-safe via downcast).
    pub fn generic_column<T: 'static>(&self, idx: usize) -> Option<&GenericColumn<T>> {
        self.generic_columns
            .get(idx)?
            .as_any_ref()
            .downcast_ref::<GenericColumn<T>>()
    }

    /// Mutable access to a generic column by index.
    pub fn generic_column_mut<T: 'static>(&mut self, idx: usize) -> Option<&mut GenericColumn<T>> {
        self.generic_columns
            .get_mut(idx)?
            .as_any_mut()
            .downcast_mut::<GenericColumn<T>>()
    }

    /// Push a new generic column onto this page (used during cell construction).
    pub(crate) fn push_generic_column(&mut self, col: Box<dyn GenericColumnAny>) {
        self.generic_columns.push(col);
    }

    /// Iterate generic columns (for type-erased operations in CellStorage).
    pub(crate) fn generic_columns(&self) -> &[Box<dyn GenericColumnAny>] {
        &self.generic_columns
    }

    /// Mutable iteration over generic columns.
    pub(crate) fn generic_columns_mut(&mut self) -> &mut [Box<dyn GenericColumnAny>] {
        &mut self.generic_columns
    }

    /// Raw pointer to a column's first element (for tests / future SIMD).
    pub fn column_ptr(&self, col: usize) -> *const u8 {
        // SAFETY: offset is within the allocation by PageLayout construction.
        unsafe { self.data.add(self.layout.column_offsets[col]) }
    }

    /// Mutable raw pointer to a column's first element.
    pub(crate) fn column_ptr_mut(&mut self, col: usize) -> *mut u8 {
        // SAFETY: offset is within the allocation by PageLayout construction.
        unsafe { self.data.add(self.layout.column_offsets[col]) }
    }
}

/// Typed column access — a view of all `capacity` slots (including dead rows;
/// callers filter through liveness/len). Panics if `T`'s size doesn't match
/// the registered `ColumnDesc` — the M1b TypeToken layer makes this statically
/// safe; for now the size check guards against mis-typed access.
impl Page {
    pub fn column_slice<T: Pod>(&self, col: usize) -> &[T] {
        let len = self.assert_column::<T>(col);
        // SAFETY: column region holds `capacity` elements of size_of::<T>()
        // bytes, 64-byte aligned (≥ align_of::<T>(), enforced at layout
        // build), zero-initialised (valid for T: Pod), borrowed under &self.
        unsafe { std::slice::from_raw_parts(self.column_ptr(col) as *const T, len) }
    }

    pub fn column_slice_mut<T: Pod>(&mut self, col: usize) -> &mut [T] {
        let len = self.assert_column::<T>(col);
        let ptr = self.column_ptr_mut(col) as *mut T;
        // SAFETY: as column_slice, under &mut self with a *mut derived from &mut self.
        unsafe { std::slice::from_raw_parts_mut(ptr, len) }
    }

    /// Raw bytes of a Pod column up to `rows` elements (for GPU sync).
    pub fn column_raw_bytes(&self, col: usize, rows: u32) -> &[u8] {
        let desc = self.layout.column_descs[col];
        let byte_len = desc.size as usize * rows as usize;
        unsafe { std::slice::from_raw_parts(self.column_ptr(col), byte_len) }
    }

    /// Validates the column's element size matches `T` and returns the slice length.
    #[inline]
    fn assert_column<T>(&self, col: usize) -> usize {
        let desc = self.layout.column_descs[col];
        assert_eq!(
            desc.size as usize,
            std::mem::size_of::<T>(),
            "column type size mismatch"
        );
        self.layout.capacity as usize
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        // SAFETY: data was allocated with alloc_layout in Page::new.
        unsafe { dealloc(self.data, self.alloc_layout) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_column_layout() -> PageLayout {
        // column 0: u64 entity ids; column 1: f32 bounds-min-x
        PageLayout::new(&[ColumnDesc::of::<u64>(), ColumnDesc::of::<f32>()], 256)
            .expect("layout fits stride budget")
    }

    #[test]
    fn columns_are_64_byte_aligned() {
        let page = Page::new(&two_column_layout());
        for col in 0..2 {
            let ptr = page.column_ptr(col) as usize;
            assert_eq!(ptr % 64, 0, "column {col} must start on a cache line");
        }
    }

    #[test]
    fn capacity_default_and_ceiling() {
        assert!(PageLayout::new(&[ColumnDesc::of::<u64>()], 1024).is_ok());
        assert!(PageLayout::new(&[ColumnDesc::of::<u64>()], 1025).is_err());
        assert!(PageLayout::new(&[ColumnDesc::of::<u64>()], 0).is_err());
    }

    #[test]
    fn stride_guardrail_128_bytes() {
        // 16 u64 columns = 128 bytes/element → ok; 17 → reject (C2).
        let cols: Vec<ColumnDesc> = (0..16).map(|_| ColumnDesc::of::<u64>()).collect();
        assert!(PageLayout::new(&cols, 256).is_ok());
        let cols: Vec<ColumnDesc> = (0..17).map(|_| ColumnDesc::of::<u64>()).collect();
        assert!(matches!(
            PageLayout::new(&cols, 256),
            Err(LayoutError::StrideExceeded { stride: 136 })
        ));
    }

    #[test]
    fn over_aligned_column_rejected() {
        #[repr(align(128))]
        #[derive(Copy, Clone)]
        struct Over(u8);
        // 128-byte alignment exceeds the 64-byte column boundary.
        assert!(matches!(
            PageLayout::new(&[ColumnDesc::of::<Over>()], 16),
            Err(LayoutError::AlignmentExceeded { align: 128 })
        ));
    }

    #[test]
    fn column_write_read_roundtrip() {
        let layout = two_column_layout();
        let mut page = Page::new(&layout);
        {
            let ids = page.column_slice_mut::<u64>(0);
            ids[0] = 0xDEAD_BEEF;
            ids[255] = 42;
        }
        {
            let xs = page.column_slice_mut::<f32>(1);
            xs[0] = -1.5;
        }
        let ids = page.column_slice::<u64>(0);
        assert_eq!(ids[0], 0xDEAD_BEEF);
        assert_eq!(ids[255], 42);
        let xs = page.column_slice::<f32>(1);
        assert_eq!(xs[0], -1.5);
    }

    #[test]
    fn len_starts_zero_capacity_from_layout() {
        let page = Page::new(&two_column_layout());
        assert_eq!(page.len(), 0);
        assert_eq!(page.capacity(), 256);
    }

    #[test]
    #[should_panic(expected = "column type size mismatch")]
    fn wrong_element_size_panics() {
        let page = Page::new(&two_column_layout());
        let _ = page.column_slice::<u32>(0); // column 0 is u64
    }

    #[test]
    fn mat4_array_is_a_column_type() {
        // [f32; 16] is a Pod column element (C5 instance mat4); size must be 64.
        let d = ColumnDesc::of::<[f32; 16]>();
        assert_eq!(d.size, 64);
    }
}
