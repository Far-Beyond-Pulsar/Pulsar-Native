use std::sync::atomic::{AtomicUsize, Ordering};
use std::any::TypeId;
pub static COUNTER: AtomicUsize = AtomicUsize::new(0);
thread_local! { pub static TL: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub struct Marker(pub u32);
pub fn bump() -> usize { COUNTER.fetch_add(1, Ordering::SeqCst) + 1 }
pub fn marker_type_id() -> TypeId { TypeId::of::<Marker>() }
pub fn counter_addr() -> usize { &COUNTER as *const _ as usize }
pub fn tl_addr() -> usize { TL.with(|c| c as *const _ as usize) }
pub fn generic_addr<T>() -> usize { generic::<T> as usize }
fn generic<T>() {}
