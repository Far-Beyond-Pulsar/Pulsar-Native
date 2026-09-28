//! Hot-reloadable native libraries.
//!
//! A native library is a dynamic library that registers natives through a
//! single entry point, declared with [`native_library!`](crate::native_library):
//!
//! ```ignore
//! fn register(registrar: &mut LibraryRegistrar) {
//!     registrar.add(NativeFn::builder("mygame::double").pure().build(|x: i64| x * 2));
//! }
//! pulsar_script_vm::native_library!(register);
//! ```
//!
//! [`NativeLibraries`] loads, reloads and unloads them against a
//! [`NativeRegistry`]:
//!
//! - Each load maps a **shadow copy** of the file, so the original can be
//!   rebuilt while loaded and a reload never gets the old mapping back from
//!   the loader's cache.
//! - The library must be built against the same `pulsar_script_vm` (and
//!   compiler) as the engine; this is checked by comparing the `TypeId` of
//!   [`LibraryRegistrar`] across the boundary, which differs between builds.
//! - Every native keeps its library mapped (an `Arc`), so programs linked
//!   against an old version keep working until they are relinked; the old
//!   code is unmapped when the last one is dropped.
//! - Reloading removes the library's natives and registers the new build's,
//!   bumping the registry generation. Relinking then re-verifies every
//!   import, so a changed signature is a link error, never a call through a
//!   stale pointer.
//!
//! Libraries may register natives but not types (see [`crate::types`]).
//!
//! Memory crosses the boundary (e.g. strings a native returns), so a
//! library must allocate with the engine's allocator. `native_library!`
//! installs a [`ForwardingAllocator`] as the library's global allocator,
//! and the loader hands it the host's [`HostAllocator`] before calling
//! anything else in the library. Every allocation the library makes then
//! goes through the engine's global allocator (and shows up in its memory
//! tracking), so memory can be freed on either side.

use std::alloc::{GlobalAlloc, Layout, System};
use std::any::TypeId;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::Arc;

use crate::native::{DuplicateNative, NativeFn, NativeRegistry, Origin};

/// Identifies a loaded library across reloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LibraryId(pub u32);

/// Collects the natives a library registers.
#[derive(Default)]
pub struct LibraryRegistrar {
    natives: Vec<NativeFn>,
}

impl LibraryRegistrar {
    pub fn add(&mut self, native: NativeFn) {
        self.natives.push(native);
    }
}

/// The host's allocation functions, handed to a library at load time.
#[repr(C)]
pub struct HostAllocator {
    pub alloc: unsafe extern "C" fn(size: usize, align: usize) -> *mut u8,
    pub alloc_zeroed: unsafe extern "C" fn(size: usize, align: usize) -> *mut u8,
    pub dealloc: unsafe extern "C" fn(ptr: *mut u8, size: usize, align: usize),
    pub realloc: unsafe extern "C" fn(ptr: *mut u8, size: usize, align: usize, new_size: usize) -> *mut u8,
}

impl HostAllocator {
    /// The global allocator of the binary this is called from.
    pub const fn global() -> &'static HostAllocator {
        unsafe extern "C" fn alloc(size: usize, align: usize) -> *mut u8 {
            std::alloc::alloc(Layout::from_size_align_unchecked(size, align))
        }
        unsafe extern "C" fn alloc_zeroed(size: usize, align: usize) -> *mut u8 {
            std::alloc::alloc_zeroed(Layout::from_size_align_unchecked(size, align))
        }
        unsafe extern "C" fn dealloc(ptr: *mut u8, size: usize, align: usize) {
            std::alloc::dealloc(ptr, Layout::from_size_align_unchecked(size, align))
        }
        unsafe extern "C" fn realloc(ptr: *mut u8, size: usize, align: usize, new_size: usize) -> *mut u8 {
            std::alloc::realloc(ptr, Layout::from_size_align_unchecked(size, align), new_size)
        }
        static GLOBAL: HostAllocator = HostAllocator { alloc, alloc_zeroed, dealloc, realloc };
        &GLOBAL
    }
}

/// A library's global allocator: forwards to the [`HostAllocator`] the
/// loader installs, and to the system allocator until then (the loader
/// installs it before calling anything else in the library, so only code
/// that runs while the library is mapped, such as initializers, can see
/// the fallback).
pub struct ForwardingAllocator {
    host: AtomicPtr<HostAllocator>,
}

impl ForwardingAllocator {
    pub const fn new() -> Self {
        Self { host: AtomicPtr::new(std::ptr::null_mut()) }
    }

    /// Route every later allocation to `host`.
    pub fn install(&self, host: &'static HostAllocator) {
        self.host.store(host as *const HostAllocator as *mut HostAllocator, Ordering::Release);
    }

    #[inline]
    fn host(&self) -> Option<&'static HostAllocator> {
        // SAFETY: only ever set from a `&'static HostAllocator`.
        unsafe { self.host.load(Ordering::Acquire).as_ref() }
    }
}

impl Default for ForwardingAllocator {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for ForwardingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match self.host() {
            Some(host) => (host.alloc)(layout.size(), layout.align()),
            None => System.alloc(layout),
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        match self.host() {
            Some(host) => (host.alloc_zeroed)(layout.size(), layout.align()),
            None => System.alloc_zeroed(layout),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match self.host() {
            Some(host) => (host.dealloc)(ptr, layout.size(), layout.align()),
            None => System.dealloc(ptr, layout),
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        match self.host() {
            Some(host) => (host.realloc)(ptr, layout.size(), layout.align(), new_size),
            None => System.realloc(ptr, layout, new_size),
        }
    }
}

/// Symbol names of a library's entry points.
pub const ABI_SYMBOL: &[u8] = b"pulsar_script_library_abi";
pub const REGISTER_SYMBOL: &[u8] = b"pulsar_script_library_register";
/// Optional: present when the library forwards allocations to the host.
pub const SET_ALLOCATOR_SYMBOL: &[u8] = b"pulsar_script_library_set_allocator";

/// Declare this crate's native library entry point. `$register` is a
/// `fn(&mut LibraryRegistrar)`.
///
/// This also makes the crate's global allocator a [`ForwardingAllocator`],
/// so the library allocates with the engine's allocator. A crate that also
/// links into a binary with its own `#[global_allocator]` can opt out with
/// `native_library!($register, without_host_allocator)`; it must then use
/// the same allocator as the engine by other means.
#[macro_export]
macro_rules! native_library {
    ($register:path) => {
        #[global_allocator]
        static PULSAR_SCRIPT_LIBRARY_ALLOCATOR: $crate::library::ForwardingAllocator =
            $crate::library::ForwardingAllocator::new();

        #[doc(hidden)]
        #[no_mangle]
        pub fn pulsar_script_library_set_allocator(host: &'static $crate::library::HostAllocator) {
            PULSAR_SCRIPT_LIBRARY_ALLOCATOR.install(host)
        }

        $crate::native_library!($register, without_host_allocator);
    };
    ($register:path, without_host_allocator) => {
        #[doc(hidden)]
        #[no_mangle]
        pub fn pulsar_script_library_abi() -> ::std::any::TypeId {
            ::std::any::TypeId::of::<$crate::library::LibraryRegistrar>()
        }

        #[doc(hidden)]
        #[no_mangle]
        pub fn pulsar_script_library_register(registrar: &mut $crate::library::LibraryRegistrar) {
            $register(registrar)
        }
    };
}

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("could not copy `{path}` for loading: {source}")]
    Copy { path: PathBuf, source: std::io::Error },
    #[error("could not load `{path}`: {source}")]
    Load { path: PathBuf, source: libloading::Error },
    #[error("`{path}` is not a script native library (missing `{symbol}`)")]
    NotALibrary { path: PathBuf, symbol: String },
    #[error("`{path}` was built against a different pulsar_script_vm or compiler")]
    AbiMismatch { path: PathBuf },
    #[error(transparent)]
    Duplicate(#[from] DuplicateNative),
    #[error("no library with id {0:?} is loaded")]
    NotLoaded(LibraryId),
}

/// A mapped shadow copy of a library file; deletes the copy once unmapped.
pub struct ShadowLibrary {
    library: ManuallyDrop<libloading::Library>,
    path: PathBuf,
}

impl Drop for ShadowLibrary {
    fn drop(&mut self) {
        // SAFETY: dropped exactly once, here; nothing uses it afterwards.
        unsafe { ManuallyDrop::drop(&mut self.library) };
        let _ = std::fs::remove_file(&self.path);
    }
}

struct Loaded {
    source: PathBuf,
    natives: Vec<String>,
}

/// The native libraries loaded into one [`NativeRegistry`].
pub struct NativeLibraries {
    loaded: HashMap<LibraryId, Loaded>,
    next_id: u32,
    shadow_dir: PathBuf,
    allocator: &'static HostAllocator,
}

static SHADOW_COUNTER: AtomicU64 = AtomicU64::new(0);

impl NativeLibraries {
    /// Shadow copies go in `shadow_dir` (created if missing).
    pub fn new(shadow_dir: impl Into<PathBuf>) -> Self {
        Self {
            loaded: HashMap::new(),
            next_id: 1,
            shadow_dir: shadow_dir.into(),
            allocator: HostAllocator::global(),
        }
    }

    /// Libraries loaded from now on allocate through `allocator` instead of
    /// this binary's global allocator.
    pub fn with_allocator(mut self, allocator: &'static HostAllocator) -> Self {
        self.allocator = allocator;
        self
    }

    /// Load the library at `path` and register its natives.
    pub fn load(
        &mut self,
        path: impl AsRef<Path>,
        registry: &mut NativeRegistry,
    ) -> Result<LibraryId, LibraryError> {
        let id = LibraryId(self.next_id);
        let natives = self.open(path.as_ref(), id)?;
        let names = Self::install(registry, natives)?;
        self.next_id += 1;
        self.loaded.insert(id, Loaded { source: path.as_ref().to_owned(), natives: names });
        Ok(id)
    }

    /// Load the current build of library `id` from its original path and
    /// swap its natives. If the new build fails to load, the old natives
    /// stay registered.
    pub fn reload(
        &mut self,
        id: LibraryId,
        registry: &mut NativeRegistry,
    ) -> Result<(), LibraryError> {
        let source = self.loaded.get(&id).ok_or(LibraryError::NotLoaded(id))?.source.clone();
        let natives = self.open(&source, id)?;
        registry.remove_origin(Origin::Library(id));
        let names = Self::install(registry, natives)?;
        self.loaded.get_mut(&id).expect("checked above").natives = names;
        Ok(())
    }

    /// Unregister library `id`'s natives. Its code stays mapped until
    /// every program linked against it is dropped.
    pub fn unload(&mut self, id: LibraryId, registry: &mut NativeRegistry) -> Result<(), LibraryError> {
        self.loaded.remove(&id).ok_or(LibraryError::NotLoaded(id))?;
        registry.remove_origin(Origin::Library(id));
        Ok(())
    }

    /// Names of the natives library `id` currently provides.
    pub fn natives(&self, id: LibraryId) -> Option<&[String]> {
        self.loaded.get(&id).map(|l| l.natives.as_slice())
    }

    /// Register `natives` all-or-nothing.
    fn install(
        registry: &mut NativeRegistry,
        natives: Vec<NativeFn>,
    ) -> Result<Vec<String>, LibraryError> {
        if let Some(dup) = natives.iter().find(|n| registry.get(&n.name).is_some()) {
            return Err(DuplicateNative(dup.name.clone()).into());
        }
        let mut names = Vec::with_capacity(natives.len());
        for native in natives {
            names.push(native.name.clone());
            registry.register(native)?;
        }
        Ok(names)
    }

    /// Map a shadow copy of `path` and collect its natives.
    fn open(&self, path: &Path, id: LibraryId) -> Result<Vec<NativeFn>, LibraryError> {
        std::fs::create_dir_all(&self.shadow_dir)
            .map_err(|source| LibraryError::Copy { path: path.to_owned(), source })?;
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let shadow = self.shadow_dir.join(format!(
            "{}-{}-{}-{file_name}",
            std::process::id(),
            id.0,
            SHADOW_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::copy(path, &shadow)
            .map_err(|source| LibraryError::Copy { path: path.to_owned(), source })?;

        // SAFETY: loading runs the library's initializers. Native libraries
        // are trusted engine extensions, like editor plugins.
        let library = match unsafe { libloading::Library::new(&shadow) } {
            Ok(library) => library,
            Err(source) => {
                let _ = std::fs::remove_file(&shadow);
                return Err(LibraryError::Load { path: path.to_owned(), source });
            }
        };
        let library = Arc::new(ShadowLibrary { library: ManuallyDrop::new(library), path: shadow });

        let not_a_library = |symbol: &[u8]| LibraryError::NotALibrary {
            path: path.to_owned(),
            symbol: String::from_utf8_lossy(symbol).into_owned(),
        };
        // SAFETY: the symbols are declared by `native_library!` with exactly
        // these types; the ABI check below (a `TypeId` that differs between
        // builds) runs before anything else is called.
        let natives = unsafe {
            let abi: libloading::Symbol<'_, fn() -> TypeId> =
                library.library.get(ABI_SYMBOL).map_err(|_| not_a_library(ABI_SYMBOL))?;
            if abi() != TypeId::of::<LibraryRegistrar>() {
                return Err(LibraryError::AbiMismatch { path: path.to_owned() });
            }
            // Before anything that allocates.
            if let Ok(set_allocator) =
                library.library.get::<fn(&'static HostAllocator)>(SET_ALLOCATOR_SYMBOL)
            {
                set_allocator(self.allocator);
            }
            let register: libloading::Symbol<'_, fn(&mut LibraryRegistrar)> =
                library.library.get(REGISTER_SYMBOL).map_err(|_| not_a_library(REGISTER_SYMBOL))?;
            let mut registrar = LibraryRegistrar::default();
            register(&mut registrar);
            registrar.natives
        };

        Ok(natives
            .into_iter()
            .map(|mut native| {
                native.attach_library(id, Arc::clone(&library));
                native
            })
            .collect())
    }
}
