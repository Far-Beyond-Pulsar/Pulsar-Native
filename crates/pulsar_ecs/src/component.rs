use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

// ── ComponentId registry ─────────────────────────────────────────────────────
//
// Every component type in the ECS gets a dense u32 ComponentId assigned on
// first access.  A thread-local cache provides a fast read path for the hot
// query loop (no atomic ops, no syscalls); the global Mutex-backed registry
// handles the cold registration path and cross-thread consistency.
//
// NOTE: a per-monomorphisation OnceLock approach was tried but triggers
// linker ICF (identical-code folding) on macOS, which merges the statics
// across different T → CID collisions.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(pub u32);

// ID 0 is reserved — columns are stored with id ≥ 1.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn registry() -> &'static Mutex<Vec<TypeId>> {
    static REG: OnceLock<Mutex<Vec<TypeId>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Vec::new()))
}

thread_local! {
    /// Per-thread cache: maps TypeId → ComponentId.  Rebuilt lazily on
    /// first access per type per thread.  Uses a plain Vec and linear scan
    /// because the number of component types is tiny (typically < 32).
    static CID_CACHE: RefCell<Vec<(TypeId, ComponentId)>> = const { RefCell::new(Vec::new()) };
}

/// Returns the canonical `ComponentId` for `T`, registering it on first call.
///
/// Hot path: thread-local cache lookup (linear scan of ~8–20 entries).
/// Cold path: acquire the global Mutex once per type per thread.
pub fn component_id<T: 'static>() -> ComponentId {
    let tid = TypeId::of::<T>();
    // Fast path — thread-local, no synchronization.
    if let Some(cid) = CID_CACHE.with(|cache| {
        cache.borrow().iter().find(|&&(t, _)| t == tid).map(|&(_, c)| c)
    }) {
        return cid;
    }
    // Slow path — register globally, then cache locally.
    let mut reg = registry().lock().expect("ComponentId registry lock");
    for (i, &rtid) in reg.iter().enumerate() {
        if rtid == tid {
            let cid = ComponentId(i as u32 + 1);
            CID_CACHE.with(|cache| cache.borrow_mut().push((tid, cid)));
            return cid;
        }
    }
    let cid = ComponentId(reg.len() as u32 + 1);
    reg.push(tid);
    NEXT_ID.store(cid.0 + 1, Ordering::Relaxed);
    CID_CACHE.with(|cache| cache.borrow_mut().push((tid, cid)));
    cid
}

/// Resolve a TypeId that was already registered. Panics if not registered.
pub fn resolve_id(type_id: TypeId) -> ComponentId {
    let reg = registry().lock().expect("ComponentId registry lock");
    for (i, &tid) in reg.iter().enumerate() {
        if tid == type_id {
            return ComponentId(i as u32 + 1);
        }
    }
    panic!("TypeId {:?} is not registered as a component", type_id);
}

/// Total number of registered component types.
pub fn component_count() -> u32 {
    let reg = registry().lock().expect("ComponentId registry lock");
    reg.len() as u32
}

/// Get the TypeId for a given ComponentId (used for downcast).
pub fn type_of(id: ComponentId) -> TypeId {
    let reg = registry().lock().expect("ComponentId registry lock");
    reg[id.0 as usize - 1]
}

// ── Component trait ──────────────────────────────────────────────────────────

pub trait Component: Any + Send + Sync + 'static {}
impl<T: Any + Send + Sync + 'static> Component for T {}

// ── Column storage ───────────────────────────────────────────────────────────

pub(crate) trait ErasedColumn: Any + Send + Sync {
    fn type_id(&self) -> TypeId;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove the value at `row` (swap-remove semantics, returns a
    /// heap-allocated pointer to the moved-out value).  The caller is
    /// responsible for dropping the returned pointer.
    ///
    /// # Safety
    /// - `row` must be < `self.len()`.
    unsafe fn swap_remove_erased(&mut self, row: usize) -> *mut ();
    /// Push a value previously obtained from `swap_remove_erased` into this
    /// column.  Takes ownership of the pointer.
    ///
    /// # Safety
    /// - `ptr` must be a valid, properly-aligned, heap-allocated value of the
    ///   concrete type stored in this column.
    unsafe fn push_erased(&mut self, ptr: *mut ());
    /// Drop the value at `ptr` (a raw pointer returned by
    /// `swap_remove_erased`).  Only the value is dropped — the allocation
    /// itself is freed.
    ///
    /// # Safety
    /// - `ptr` must be a valid, properly-aligned, heap-allocated value of the
    ///   concrete type stored in this column.
    unsafe fn drop_erased(&self, ptr: *mut ());

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    fn new_empty(&self) -> Box<dyn ErasedColumn>;
}

pub(crate) struct Column<T: Component> {
    pub data: Vec<T>,
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

    // SAFETY: row < self.data.len() (caller guarantees)
    unsafe fn swap_remove_erased(&mut self, row: usize) -> *mut () {
        // SAFETY: caller guarantees row is in bounds
        let val = self.data.swap_remove(row);
        // Box + leak to get a stable *mut () that the caller can push or drop.
        Box::into_raw(Box::new(val)) as *mut ()
    }

    // SAFETY: ptr is a valid Box<T> for this column's T (caller guarantees)
    unsafe fn push_erased(&mut self, ptr: *mut ()) {
        // SAFETY: recreate the Box from the raw pointer so Drop runs
        // after the value is moved into the Vec.
        let val = *Box::from_raw(ptr as *mut T);
        self.data.push(val);
    }

    // SAFETY: ptr is a valid Box<T> for this column's T (caller guarantees)
    unsafe fn drop_erased(&self, ptr: *mut ()) {
        // SAFETY: reconstruct Box to run Drop, then it falls out of scope.
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
