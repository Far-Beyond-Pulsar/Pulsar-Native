//! Permanent library loading wrapper.
//!
//! This module provides `PermanentLibrary`, a wrapper around `libloading::Library`
//! that prevents dynamic library unloading to eliminate undefined behavior.
//!
//! ## Why This Exists
//!
//! Unloading dynamic libraries that share Rust types with the host application
//! causes undefined behavior:
//!
//! 1. **Drop glue**: `Arc<T>` stores function pointers to `T::drop` in the library
//! 2. **Vtables**: Trait objects store vtable pointers in the library's `.rodata`
//! 3. **Function pointers**: Callbacks and closures point to library code
//!
//! If we call `dlclose`/`FreeLibrary`, all these sections are unmapped, causing
//! segfaults when the pointers are accessed.
//!
//! ## The Solution
//!
//! `PermanentLibrary` wraps the library handle in `ManuallyDrop` and provides no
//! way to unload it. The library stays loaded for the process lifetime, ensuring
//! all pointers remain valid.
//!
//! ## Safety Contract
//!
//! Once a `PermanentLibrary` is created:
//! - The library remains loaded until process termination
//! - All symbols (functions, vtables, drop glue) remain valid
//! - It is safe to store function pointers from the library
//! - It is safe to create `Arc<T>` where `T`'s drop glue is in the library
//! - It is safe to create trait objects whose vtables are in the library
//!
//! ## Example
//!
//! ```rust,ignore
//! use plugin_manager::PermanentLibrary;
//!
//! // Load a plugin library (never unloads!)
//! let lib = PermanentLibrary::new("plugins/my_plugin.dll")?;
//!
//! // Get symbols - they remain valid forever
//! let create_fn: Symbol<PluginCreate> = unsafe { lib.get(b"_plugin_create")? };
//! let plugin = unsafe { create_fn() };
//!
//! // Safe to share Arc across boundary because library never unloads
//! let panel: Arc<dyn PanelView> = plugin.create_editor(...)?;
//! ```

use libloading::{Library, Symbol};
use sha2::{Digest, Sha256};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

/// A dynamically loaded library that is NEVER unloaded.
///
/// This wrapper around `libloading::Library` prevents undefined behavior by:
/// 1. Keeping the library loaded for the process lifetime
/// 2. Ensuring all function pointers, vtables, and drop glue remain valid
/// 3. Allowing safe sharing of `Arc<T>` and trait objects across the boundary
///
/// # Safety Contract
///
/// Once a `PermanentLibrary` is created:
/// - The library will remain loaded until process termination
/// - All symbols remain valid indefinitely
/// - It is safe to store function pointers from this library
/// - It is safe to create `Arc<T>` where `T`'s drop glue is in this library
/// - It is safe to create trait objects whose vtables are in this library
///
/// # Memory Leak
///
/// Yes, this "leaks" the library handle. This is intentional and necessary for safety.
/// The OS will clean up the memory when the process exits.
///
/// # Platform Support
///
/// Works on all platforms supported by `libloading`:
/// - Windows: Uses `LoadLibraryW` (never calls `FreeLibrary`)
/// - Linux: Uses `dlopen` (never calls `dlclose`)
/// - macOS: Uses `dlopen` (never calls `dlclose`)
#[derive(Debug)]
pub struct PermanentLibrary {
    /// The underlying library handle.
    ///
    /// SAFETY: Wrapped in `ManuallyDrop` to prevent automatic unloading.
    /// We intentionally leak this to keep the library loaded forever.
    ///
    /// The library contains:
    /// - `.text`: Code (functions, drop glue)
    /// - `.rodata`: Read-only data (vtables, static strings)
    /// - `.data`: Mutable static data
    ///
    /// All of these sections remain mapped for process lifetime.
    library: ManuallyDrop<Library>,

    /// Path to the library (for debugging/logging).
    path: PathBuf,

    /// SHA-256 digest of the library file at load time.
    /// Used for integrity verification; checked against manifests.
    sha256: [u8; 32],
}

impl PermanentLibrary {
    /// Load a library and mark it as permanent (never unloaded).
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the dynamic library (.dll/.so/.dylib)
    ///
    /// # Returns
    ///
    /// Returns `Ok(PermanentLibrary)` if the library was loaded successfully.
    ///
    /// # Errors
    ///
    /// Returns `Err` if:
    /// - The file does not exist
    /// - The file is not a valid dynamic library
    /// - The library architecture doesn't match (e.g., loading 32-bit lib in 64-bit process)
    /// - The library has missing dependencies
    /// - The OS denies permission to load the library
    ///
    /// # Safety
    ///
    /// This function is safe to call, but the library code itself must be trusted.
    /// The library will be loaded once and never unloaded.
    ///
    /// # Platform-Specific Behavior
    ///
    /// **Windows:**
    /// - Uses `LoadLibraryW` to load the library
    /// - Searches in: executable dir, system dirs, PATH
    /// - Dependencies must be in same directory or on PATH
    ///
    /// **Linux:**
    /// - Uses `dlopen` with `RTLD_LAZY | RTLD_LOCAL`
    /// - Searches in: LD_LIBRARY_PATH, DT_RPATH, DT_RUNPATH, /lib, /usr/lib
    /// - Dependencies resolved via `DT_NEEDED` entries
    ///
    /// **macOS:**
    /// - Uses `dlopen` with `RTLD_LAZY | RTLD_LOCAL`
    /// - Searches in: DYLD_LIBRARY_PATH, @rpath, @executable_path, /usr/lib
    /// - Dependencies use install names (check with `otool -L`)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let lib = PermanentLibrary::new("plugins/example.dll")?;
    /// // Library is now loaded and will stay loaded until process exits
    /// ```
    pub fn new(path: impl AsRef<Path>) -> Result<Self, libloading::Error> {
        let path = path.as_ref();

        // Open the file, hash it while holding the handle, then load from the
        // same handle.  This prevents a TOCTOU race where the file is swapped
        // between hash computation and dlopen/LoadLibrary.
        //
        // On Linux we use /proc/self/fd/N to load from the open fd so that
        // the kernel guarantees we load the same bytes we hashed.  On other
        // platforms we fall back to the path (which is still better than
        // hashing then loading from the path with no locking at all).
        let file = std::fs::File::open(path).map_err(|e| {
            tracing::warn!("Failed to open library '{}': {}", path.display(), e);
            libloading::Error::IncompatibleSize
        })?;

        // Hash from the open file handle.
        let sha256 = Self::compute_file_hash_from_handle(&file).map_err(|e| {
            tracing::warn!("Failed to hash library '{}': {}", path.display(), e);
            libloading::Error::IncompatibleSize
        })?;

        // Drop the file handle on Windows before loading (LoadLibraryW needs
        // exclusive access).  On Unix we keep the fd open and use /proc/self/fd.
        #[cfg(target_os = "linux")]
        let fd_path = {
            use std::os::fd::AsRawFd;
            let fd = file.as_raw_fd();
            std::path::PathBuf::from(format!("/proc/self/fd/{}", fd))
        };
        // Keep `file` alive on Linux, drop it everywhere else.
        #[cfg(not(target_os = "linux"))]
        drop(file);

        // SAFETY: We load the library normally using libloading::Library.
        // The library must be a valid dynamic library for the current platform
        // and architecture. libloading will return an error if it's not.
        //
        // On Linux we load from /proc/self/fd/N so the kernel uses the same
        // inode that we hashed, preventing TOCTOU.
        let load_path: &std::path::Path = path;
        #[cfg(target_os = "linux")]
        let load_path: &std::path::Path = &fd_path;

        let library = unsafe { Library::new(load_path)? };

        tracing::info!(
            "Loaded permanent library: {:?} (sha256={:02x?}, will never unload)",
            path,
            sha256
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>()
        );

        Ok(Self {
            library: ManuallyDrop::new(library),
            path: path.to_path_buf(),
            sha256,
        })
    }

    /// Compute the SHA-256 digest of a file on disk.
    fn compute_file_hash(path: &Path) -> Result<[u8; 32], std::io::Error> {
        let bytes = std::fs::read(path)?;
        Ok(Sha256::digest(&bytes).into())
    }

    /// Compute the SHA-256 digest from an open file handle.
    /// Used by `new()` to hash the file before loading, preventing TOCTOU races.
    fn compute_file_hash_from_handle(file: &std::fs::File) -> Result<[u8; 32], std::io::Error> {
        use std::io::Read;
        let mut reader = std::io::BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(hasher.finalize().into())
    }

    /// Get a symbol from the library.
    ///
    /// # Type Parameter
    ///
    /// `T` is the type of the symbol. For functions, this is typically the function
    /// signature. For example:
    /// - `extern "C" fn() -> i32` for a C function returning int
    /// - `extern "C" fn(*const u8) -> *mut Foo` for a constructor
    ///
    /// # Arguments
    ///
    /// * `symbol` - The symbol name as a byte string (e.g., `b"my_function"`)
    ///
    /// # Returns
    ///
    /// Returns `Ok(Symbol<T>)` if the symbol exists and has the correct type.
    ///
    /// # Errors
    ///
    /// Returns `Err` if:
    /// - The symbol does not exist in the library
    /// - The symbol name is not valid UTF-8 or contains null bytes
    ///
    /// # Safety
    ///
    /// The caller must ensure:
    /// 1. The symbol name is correct and exists in the library
    /// 2. The symbol type `T` matches the actual symbol type in the library
    /// 3. The symbol is safe to call/use according to its contract
    ///
    /// Because the library is never unloaded, the returned symbol reference
    /// is valid for the entire process lifetime. This means:
    /// - Function pointers can be stored indefinitely
    /// - Data pointers remain valid
    /// - You can create `'static` references (but be careful!)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Get a function symbol
    /// let create_fn: Symbol<extern "C" fn() -> *mut Plugin> = unsafe {
    ///     lib.get(b"plugin_create")?
    /// };
    ///
    /// // Call it (symbol is always valid because library never unloads)
    /// let plugin = unsafe { create_fn() };
    /// ```
    pub unsafe fn get<T>(&self, symbol: &[u8]) -> Result<Symbol<'_, T>, libloading::Error> {
        // SAFETY: Caller ensures:
        // 1. Symbol exists and name is correct
        // 2. Type T matches the actual symbol type
        // 3. Using the symbol is safe according to its contract
        //
        // The symbol will remain valid forever because we never unload the library.
        // ManuallyDrop prevents the library from being dropped, so dlclose/FreeLibrary
        // is never called.
        self.library.get(symbol)
    }

    /// Get the library path.
    ///
    /// This returns the path that was used to load the library.
    /// Useful for logging and debugging.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the SHA-256 digest of the library file at load time.
    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// Verify the library file's current on-disk hash matches the hash
    /// that was computed when the library was first loaded.
    ///
    /// A mismatch indicates the file has been tampered with since loading
    /// (e.g. a TOCTOU replacement on disk).
    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        let current = Self::compute_file_hash(&self.path)
            .map_err(|e| IntegrityError::Io(self.path.clone(), e))?;
        if current != self.sha256 {
            return Err(IntegrityError::HashMismatch {
                path: self.path.clone(),
                expected: self.sha256,
                actual: current,
            });
        }
        Ok(())
    }
}

/// Errors produced by [`PermanentLibrary::verify_integrity`].
#[derive(Debug)]
pub enum IntegrityError {
    /// Could not read the library file from disk.
    Io(PathBuf, std::io::Error),
    /// The on-disk hash differs from the load-time hash —
    /// the file was modified after loading.
    HashMismatch {
        path: PathBuf,
        expected: [u8; 32],
        actual: [u8; 32],
    },
}

impl std::fmt::Display for IntegrityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntegrityError::Io(path, e) => {
                write!(f, "Failed to read '{}': {}", path.display(), e)
            }
            IntegrityError::HashMismatch {
                path,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "Integrity check failed for '{}': expected {:02x?}, got {:02x?}",
                    path.display(),
                    expected
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>(),
                    actual
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>(),
                )
            }
        }
    }
}

impl std::error::Error for IntegrityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IntegrityError::Io(_, e) => Some(e),
            IntegrityError::HashMismatch { .. } => None,
        }
    }
}

// NO Drop IMPLEMENTATION
//
// This is intentional and critical for safety!
//
// If we implemented Drop and called dlclose/FreeLibrary:
// 1. All function pointers from this library become dangling
// 2. All vtables for trait objects become invalid
// 3. All Arc<T> drop glue pointers become invalid
// 4. SEGFAULT when any of these are accessed
//
// By NOT implementing Drop:
// 1. ManuallyDrop prevents the library from being dropped
// 2. The library stays loaded until process termination
// 3. OS cleans up the memory when process exits
// 4. All pointers remain valid for process lifetime
//
// This is not a bug - it's the core safety mechanism!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permanent_library_size() {
        // Ensure PermanentLibrary is reasonably small
        // (just a ManuallyDrop wrapper + PathBuf)
        assert!(
            std::mem::size_of::<PermanentLibrary>() < 128,
            "PermanentLibrary should be small"
        );
    }

    #[test]
    fn test_permanent_library_debug() {
        // Ensure Debug implementation exists (useful for logging)
        let _path = PathBuf::from("test.dll");
        // Can't actually create one without a real DLL, but we can
        // verify the type implements Debug
        fn assert_debug<T: std::fmt::Debug>() {}
        assert_debug::<PermanentLibrary>();
    }
}
