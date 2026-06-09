use std::any::{Any, TypeId};

pub trait Component: Any + Send + Sync + 'static {}
impl<T: Any + Send + Sync + 'static> Component for T {}

pub(crate) trait ErasedColumn: Any + Send + Sync {
    fn type_id(&self) -> TypeId;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    unsafe fn swap_remove_erased(&mut self, row: usize) -> *mut ();
    unsafe fn push_erased(&mut self, ptr: *mut ());
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

    unsafe fn swap_remove_erased(&mut self, row: usize) -> *mut () {
        let val = self.data.swap_remove(row);
        Box::into_raw(Box::new(val)) as *mut ()
    }

    unsafe fn push_erased(&mut self, ptr: *mut ()) {
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
