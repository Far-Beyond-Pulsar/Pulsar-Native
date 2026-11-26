use shared_dylib as _;
use std::any::TypeId;
fn main() {
    let path = std::env::args().nth(1).expect("plugin path");
    let lib = unsafe { libloading::Library::new(&path) }.expect("load");
    unsafe {
        let pb: libloading::Symbol<extern "C" fn() -> usize> = lib.get(b"plugin_bump").unwrap();
        let pc: libloading::Symbol<extern "C" fn() -> usize> = lib.get(b"plugin_counter_addr").unwrap();
        let ptl: libloading::Symbol<extern "C" fn() -> usize> = lib.get(b"plugin_tl_addr").unwrap();
        let pg: libloading::Symbol<extern "C" fn() -> usize> = lib.get(b"plugin_generic_addr").unwrap();
        let pt: libloading::Symbol<extern "C" fn(*mut u64)> = lib.get(b"plugin_typeid_marker").unwrap();
        let ps: libloading::Symbol<extern "C" fn(*mut u64)> = lib.get(b"plugin_typeid_string").unwrap();
        let pbox: libloading::Symbol<extern "C" fn() -> *mut shared::Marker> = lib.get(b"plugin_box_roundtrip").unwrap();
        println!("host bump -> {}", shared::bump());
        println!("plugin bump -> {}", pb());
        println!("host bump -> {}", shared::bump());
        println!("counter addr host={:#x} plugin={:#x} same={}", shared::counter_addr(), pc(), shared::counter_addr()==pc());
        println!("thread_local addr host={:#x} plugin={:#x} same={}", shared::tl_addr(), ptl(), shared::tl_addr()==ptl());
        println!("generic fn addr same={}", shared::generic_addr::<u8>()==pg());
        let mut o = [0u64;2]; pt(o.as_mut_ptr());
        let t: TypeId = std::ptr::read(o.as_ptr() as *const TypeId);
        println!("TypeId<Marker> same={}", t == shared::marker_type_id());
        ps(o.as_mut_ptr());
        let t2: TypeId = std::ptr::read(o.as_ptr() as *const TypeId);
        println!("TypeId<String> same={}", t2 == TypeId::of::<String>());
        let b = pbox();
        let bx = Box::from_raw(b); // freed in host allocator
        println!("box from plugin freed in host ok: {}", bx.0);
    }
}
