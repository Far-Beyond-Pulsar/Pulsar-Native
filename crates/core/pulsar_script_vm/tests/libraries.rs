//! Loading, reloading and unloading a real native library (the
//! `test_library` fixture, built by Cargo as a dev-dependency).

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{int, Asm, Harness};
use std::sync::atomic::{AtomicUsize, Ordering};

use pulsar_script_vm::{
    HostAllocator, Instr, LibraryError, LinkError, NativeLibraries, NativeRegistry, Origin, Param,
    Program, Type, Value,
};

fn fixture_path() -> PathBuf {
    let deps = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let prefix = format!("{}pulsar_script_vm_test_library", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let exact = deps.join(format!("{prefix}{suffix}"));
    if exact.is_file() {
        return exact;
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&deps)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(suffix))
        })
        .collect();
    assert_eq!(candidates.len(), 1, "expected one fixture library in {}: {candidates:?}", deps.display());
    candidates.remove(0)
}

fn shadow_dir(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pulsar_script_vm_{test}_{}", std::process::id()))
}

fn doubling_module() -> Asm {
    let mut asm = Asm::new();
    let double = asm.import("fixture::double", vec![Param::new(Type::Int)], Type::Int);
    asm.function("double", vec![Type::Int], Type::Int, vec![Type::Int], vec![
        Instr::CallNative { import: double, args: vec![0], dst: Some(1) },
        Instr::Return { value: Some(1) },
    ]);
    asm
}

#[test]
fn load_call_reload_unload() {
    let mut registry = NativeRegistry::with_engine_natives();
    let mut libraries = NativeLibraries::new(shadow_dir("reload"));
    let id = libraries.load(fixture_path(), &mut registry).unwrap();
    assert_eq!(registry.get("fixture::double").unwrap().origin, Origin::Library(id));

    let asm = doubling_module();
    let program = asm.link(&registry);
    let mut h = Harness::new();
    assert_eq!(h.run(&program, "double", &[int(21)]).unwrap(), int(42));

    // Reload: new natives, new generation. The old program keeps its
    // (still mapped) natives until relinked.
    let before = registry.generation();
    libraries.reload(id, &mut registry).unwrap();
    assert!(registry.generation() > before);
    assert!(program.generation() < registry.generation());
    assert_eq!(h.run(&program, "double", &[int(2)]).unwrap(), int(4));
    let relinked = asm.link(&registry);
    assert_eq!(h.run(&relinked, "double", &[int(5)]).unwrap(), int(10));
    drop(program);

    // Strings allocated by the library cross back safely.
    let mut greet = Asm::new();
    let f = greet.import("fixture::greet", vec![Param::new(Type::Str)], Type::Str);
    greet.function("greet", vec![Type::Str], Type::Str, vec![Type::Str], vec![
        Instr::CallNative { import: f, args: vec![0], dst: Some(1) },
        Instr::Return { value: Some(1) },
    ]);
    let greet = greet.link(&registry);
    assert_eq!(h.run(&greet, "greet", &[Value::from("world")]).unwrap(), Value::from("hello world"));

    // Unload: the natives are gone, so relinking fails; programs already
    // linked keep the code alive and keep working.
    libraries.unload(id, &mut registry).unwrap();
    assert!(registry.get("fixture::double").is_none());
    let err = Program::link(Arc::new(asm.module.clone()), &registry).err().unwrap();
    assert_eq!(err, LinkError::MissingNative { name: "fixture::double".into() });
    assert_eq!(h.run(&relinked, "double", &[int(1)]).unwrap(), int(2));
}

#[test]
fn a_library_cannot_shadow_existing_natives() {
    let mut registry = NativeRegistry::new();
    let mut libraries = NativeLibraries::new(shadow_dir("dup"));
    libraries.load(fixture_path(), &mut registry).unwrap();
    let err = libraries.load(fixture_path(), &mut registry).unwrap_err();
    assert!(matches!(err, LibraryError::Duplicate(_)), "{err}");
    // The failed load registered nothing.
    assert_eq!(registry.len(), 3);
}

#[test]
fn non_libraries_are_rejected() {
    let dir = shadow_dir("garbage");
    std::fs::create_dir_all(&dir).unwrap();
    let garbage = dir.join("not_a_library.bin");
    std::fs::write(&garbage, b"definitely not a shared object").unwrap();
    let mut libraries = NativeLibraries::new(dir.join("shadow"));
    let err = libraries.load(&garbage, &mut NativeRegistry::new()).unwrap_err();
    assert!(matches!(err, LibraryError::Load { .. }), "{err}");
}

static COUNTED_ALLOCS: AtomicUsize = AtomicUsize::new(0);

/// The global allocator, counting what goes through it.
static COUNTING: HostAllocator = {
    unsafe extern "C" fn alloc(size: usize, align: usize) -> *mut u8 {
        COUNTED_ALLOCS.fetch_add(1, Ordering::Relaxed);
        (HostAllocator::global().alloc)(size, align)
    }
    unsafe extern "C" fn alloc_zeroed(size: usize, align: usize) -> *mut u8 {
        COUNTED_ALLOCS.fetch_add(1, Ordering::Relaxed);
        (HostAllocator::global().alloc_zeroed)(size, align)
    }
    unsafe extern "C" fn dealloc(ptr: *mut u8, size: usize, align: usize) {
        (HostAllocator::global().dealloc)(ptr, size, align)
    }
    unsafe extern "C" fn realloc(ptr: *mut u8, size: usize, align: usize, new_size: usize) -> *mut u8 {
        COUNTED_ALLOCS.fetch_add(1, Ordering::Relaxed);
        (HostAllocator::global().realloc)(ptr, size, align, new_size)
    }
    HostAllocator { alloc, alloc_zeroed, dealloc, realloc }
};

#[test]
fn a_library_allocates_with_the_host_allocator() {
    let mut registry = NativeRegistry::new();
    let mut libraries = NativeLibraries::new(shadow_dir("alloc")).with_allocator(&COUNTING);
    libraries.load(fixture_path(), &mut registry).unwrap();
    // Registration itself allocates (the natives' names and closures).
    let after_load = COUNTED_ALLOCS.load(Ordering::Relaxed);
    assert!(after_load > 0, "registering natives did not allocate through the host");

    let mut greet = Asm::new();
    let f = greet.import("fixture::greet", vec![Param::new(Type::Str)], Type::Str);
    greet.function("greet", vec![Type::Str], Type::Str, vec![Type::Str], vec![
        Instr::CallNative { import: f, args: vec![0], dst: Some(1) },
        Instr::Return { value: Some(1) },
    ]);
    let greet = greet.link(&registry);
    // The returned string is allocated by the library, through the host,
    // and freed here by the host.
    let out = Harness::new().run(&greet, "greet", &[Value::from("host")]).unwrap();
    assert_eq!(out, Value::from("hello host"));
    assert!(COUNTED_ALLOCS.load(Ordering::Relaxed) > after_load);
}
