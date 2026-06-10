use std::alloc::{alloc_zeroed, dealloc, Layout};

/// Hard ceiling on per-page element capacity (spec §4.3).
pub const MAX_PAGE_CAPACITY: u32 = 1024;
/// Recommended default capacity (spec §4.3).
pub const DEFAULT_PAGE_CAPACITY: u32 = 256;
/// Combined per-element stride limit across all columns (spec §7.1, C2).
pub const MAX_STRIDE_BYTES: u32 = 128;
/// Every column starts on a cache-line boundary (spec §4.2).
pub const COLUMN_ALIGN: usize = 64;

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
            debug_assert!(col.align as usize <= COLUMN_ALIGN);
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
}

#[inline]
fn next_multiple(n: usize, m: usize) -> usize {
    n.div_ceil(m) * m
}

/// One SoA page: a single 64-byte-aligned contiguous allocation holding all
/// columns. `len` counts live+dead rows up to the compaction frontier; the
/// liveness bitmask (liveness.rs) tracks which are alive.
pub struct Page {
    data: *mut u8,
    layout: PageLayout,
    alloc_layout: Layout,
    len: u32,
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
        assert!(!data.is_null(), "page allocation failed");
        Self {
            data,
            layout: layout.clone(),
            alloc_layout,
            len: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> u32 {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn capacity(&self) -> u32 {
        self.layout.capacity
    }

    /// Reserve the next row, returning its index. None when full.
    pub fn push_row(&mut self) -> Option<u32> {
        if self.len >= self.layout.capacity {
            return None;
        }
        let row = self.len;
        self.len += 1;
        Some(row)
    }

    /// Drop the last row (used by swap-and-pop compaction).
    pub fn pop_row(&mut self) {
        debug_assert!(self.len > 0);
        self.len -= 1;
    }

    /// Raw pointer to a column's first element (for tests / future SIMD).
    pub fn column_ptr(&self, col: usize) -> *const u8 {
        // SAFETY: offset is within the allocation by PageLayout construction.
        unsafe { self.data.add(self.layout.column_offsets[col]) }
    }
}

/// Typed column access — a view of all `capacity` slots (including dead rows;
/// callers filter through liveness/len). Panics if `T`'s size doesn't match
/// the registered `ColumnDesc` — the M1b TypeToken layer makes this statically
/// safe; for now the size check guards against mis-typed access.
impl Page {
    pub fn column_slice<T>(&self, col: usize) -> &[T] {
        let desc = self.layout.column_descs[col];
        assert_eq!(
            desc.size as usize,
            std::mem::size_of::<T>(),
            "column type size mismatch"
        );
        // SAFETY: the column region holds `capacity` elements of `desc.size`
        // bytes, 64-byte aligned (≥ align_of::<T>()), zero-initialised, and
        // borrowed under &self.
        unsafe {
            std::slice::from_raw_parts(
                self.column_ptr(col) as *const T,
                self.layout.capacity as usize,
            )
        }
    }

    pub fn column_slice_mut<T>(&mut self, col: usize) -> &mut [T] {
        let desc = self.layout.column_descs[col];
        assert_eq!(
            desc.size as usize,
            std::mem::size_of::<T>(),
            "column type size mismatch"
        );
        // SAFETY: as column_slice, under &mut self.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.column_ptr(col) as *mut T,
                self.layout.capacity as usize,
            )
        }
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
}
