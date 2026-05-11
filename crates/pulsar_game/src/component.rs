use std::any::{Any, TypeId};

/// Marker trait every component type must implement.
///
/// Auto-implemented for any `'static + Send + Sync` type.
pub trait Component: Any + Send + Sync + 'static {}
impl<T: Any + Send + Sync + 'static> Component for T {}

/// Type-erased dense storage for a single component type within one archetype.
///
/// The pointer contract for the unsafe methods:
/// - `swap_remove_erased` → returns a heap-allocated `Box<T>` as a raw `*mut ()`.
///   The caller owns that allocation and MUST either pass it to `push_erased` on
///   a column of the same type, or free it via `drop_erased`.
/// - `push_erased` → **consumes** the `Box<T>` pointer from `swap_remove_erased`.
///   Do NOT call `drop_erased` after `push_erased`.
/// - `drop_erased` → drops a pointer that was NOT consumed by `push_erased`.
pub(crate) trait ErasedColumn: Any + Send + Sync {
    fn type_id(&self) -> TypeId;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Swap-remove element at `row`.  Returns an owning `Box<T>` as `*mut ()`.
    unsafe fn swap_remove_erased(&mut self, row: usize) -> *mut ();

    /// Push an owning `Box<T>` (from `swap_remove_erased`) into this column.
    /// Consumes the allocation.
    unsafe fn push_erased(&mut self, ptr: *mut ());

    /// Free an owning `Box<T>` pointer without inserting it anywhere.
    unsafe fn drop_erased(&self, ptr: *mut ());

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Create a new empty column of the same concrete type.
    fn new_empty(&self) -> Box<dyn ErasedColumn>;
}

/// Typed column — the only concrete implementor of `ErasedColumn`.
pub(crate) struct Column<T: Component> {
    pub(crate) data: Vec<T>,
}

impl<T: Component> Column<T> {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }
}

impl<T: Component> ErasedColumn for Column<T> {
    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    unsafe fn swap_remove_erased(&mut self, row: usize) -> *mut () {
        let val = self.data.swap_remove(row);
        Box::into_raw(Box::new(val)) as *mut ()
    }

    unsafe fn push_erased(&mut self, ptr: *mut ()) {
        // Consume the Box<T> allocation and move the value into our Vec.
        let val = *Box::from_raw(ptr as *mut T);
        self.data.push(val);
    }

    unsafe fn drop_erased(&self, ptr: *mut ()) {
        drop(Box::from_raw(ptr as *mut T));
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn new_empty(&self) -> Box<dyn ErasedColumn> {
        Box::new(Column::<T>::new())
    }
}
