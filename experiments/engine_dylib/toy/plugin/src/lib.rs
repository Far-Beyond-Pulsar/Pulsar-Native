use shared_dylib as _;
use std::any::TypeId;
#[no_mangle] pub extern "C" fn plugin_bump() -> usize { shared::bump() }
#[no_mangle] pub extern "C" fn plugin_counter_addr() -> usize { shared::counter_addr() }
#[no_mangle] pub extern "C" fn plugin_tl_addr() -> usize { shared::tl_addr() }
#[no_mangle] pub extern "C" fn plugin_generic_addr() -> usize { shared::generic_addr::<u8>() }
// TypeId is 128-bit; return as two u64 via transmute (same compiler, so same layout)
#[no_mangle] pub extern "C" fn plugin_typeid_marker(out: *mut u64) { let t = TypeId::of::<shared::Marker>(); unsafe { std::ptr::write(out as *mut TypeId, t); } }
#[no_mangle] pub extern "C" fn plugin_typeid_string(out: *mut u64) { let t = TypeId::of::<String>(); unsafe { std::ptr::write(out as *mut TypeId, t); } }
#[no_mangle] pub extern "C" fn plugin_box_roundtrip() -> *mut shared::Marker { Box::into_raw(Box::new(shared::Marker(7))) }
