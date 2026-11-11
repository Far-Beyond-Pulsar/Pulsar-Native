//! Plugin libraries: the same root `build` across a library boundary.
//!
//! A plugin built as a library (`cdylib`) links its own copy of every crate
//! it uses, this one included. Its root's `build` runs in that copy, into the
//! copy's [`PluginBuilder`]; [`export_library!`] exports what every plugin
//! root linked into the library added (one plugin, or a bundle of several:
//! a library is one or more roots and one export), as plain data
//! ([`FfiLibrary`]). The host reads it ([`import`]): each plugin's descriptor
//! joins the installed set, and each registration is taken as the host's own
//! type of the same category when both copies agree on its **layout
//! fingerprint** ([`fingerprint`]: the type's name, size, alignment,
//! category and contract version, [`Contribution::VERSION`]). A registration
//! whose fingerprint differs is refused, by name, and so is one of a
//! category the host does not know: never read as a type it is not.
//!
//! The same `build` serves a static build (`installed!`, linked into the
//! editor or a game) and a library build (`export_library!` in the crate
//! built as `cdylib`): dual build from one function.
//!
//! Limits, as for every native plugin: the library is never unloaded (its
//! code and data back the registrations), it must be built by the same
//! compiler as the host (the plugin loader checks the rustc hash first), and
//! the fingerprint checks layouts by size, alignment and name, so a change
//! that keeps all three (reordered fields of one size) must bump the
//! registration type's `VERSION`.
//!
//! How others do it: Unreal loads a module DLL and calls its
//! `StartupModule`, trusting the engine version and build ids
//! (`BuildId` in `.modules`) for ABI; COM and OSGi exchange interfaces by
//! id and version. Ours keeps Rust types across the boundary, as Unreal
//! does, and checks each registration type's layout at load, so a stale
//! library is refused with the type named instead of misread.

use std::any::Any;

use crate::{Contribution, PluginBuilder, PluginDescriptor};

/// The layout fingerprint of registration type `T`: its type name, size,
/// alignment, category and contract version, hashed (FNV-1a).
pub fn fingerprint<T: Contribution + 'static>() -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(std::any::type_name::<T>().as_bytes());
    eat(&(std::mem::size_of::<T>() as u64).to_le_bytes());
    eat(&(std::mem::align_of::<T>() as u64).to_le_bytes());
    eat(T::CATEGORY.as_bytes());
    eat(&T::VERSION.to_le_bytes());
    hash
}

/// How the host takes a library's registration of one type: submitted for
/// every registration type ([`crate::registration!`], [`crate::importer!`]).
pub struct Importer {
    pub category: &'static str,
    pub fingerprint: fn() -> u64,
    /// Add the registration at the pointer, read as the host's type.
    ///
    /// # Safety
    ///
    /// The pointer is a value of a type with this importer's fingerprint
    /// that lives for the process.
    pub import: unsafe fn(&mut PluginBuilder, *const ()),
}

inventory::collect!(Importer);

/// List `T` as a type a plugin library's registrations can be imported as
/// (for registration types not declared with [`crate::registration!`],
/// which does it itself).
#[macro_export]
macro_rules! importer {
    ($($ty:ty),* $(,)?) => {$(
        $crate::inventory::submit! {
            $crate::library::Importer {
                category: <$ty as $crate::Contribution>::CATEGORY,
                fingerprint: $crate::library::fingerprint::<$ty>,
                import: |builder, value| unsafe { $crate::add_foreign::<$ty>(builder, value) },
            }
        }
    )*};
}

/// A string across the library boundary.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FfiStr {
    pub ptr: *const u8,
    pub len: usize,
}

impl FfiStr {
    fn of(text: &'static str) -> Self {
        Self {
            ptr: text.as_ptr(),
            len: text.len(),
        }
    }

    /// # Safety
    ///
    /// Points to UTF-8 that lives for the process.
    unsafe fn get(&self) -> &'static str {
        // SAFETY: the caller's contract.
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.ptr, self.len)) }
    }
}

/// One registration a library's build added.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FfiEntry {
    pub category: FfiStr,
    pub fingerprint: u64,
    pub value: *const (),
}

/// One plugin of a library: its descriptor, its root and its registrations.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FfiPlugin {
    pub descriptor: FfiStr,
    pub root: FfiStr,
    pub entries: *const FfiEntry,
    pub len: usize,
}

/// What a library exports ([`export_library!`]).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FfiLibrary {
    pub abi: u64,
    pub plugins: *const FfiPlugin,
    pub len: usize,
}

/// The version of the [`FfiLibrary`] layout.
pub const LIBRARY_ABI: u64 = 1;

/// The symbol [`export_library!`] exports.
pub const EXPORT_SYMBOL: &str = "_pulsar_plugins_export";

/// Its signature.
pub type ExportFn = unsafe extern "C" fn(*mut FfiLibrary) -> bool;

/// Export every plugin root linked into this library, through their
/// builds: call it once in the crate built as the library (a plugin's root
/// built as `cdylib`, or a bundle crate linking several roots).
#[macro_export]
macro_rules! export_library {
    () => {
        /// What this library's plugins register ([`pulsar_plugins::library`]).
        ///
        /// # Safety
        ///
        /// `out` is valid for a write.
        #[no_mangle]
        pub unsafe extern "C" fn _pulsar_plugins_export(
            out: *mut $crate::library::FfiLibrary,
        ) -> bool {
            match ::std::panic::catch_unwind($crate::library::exported) {
                Ok(library) => {
                    // SAFETY: the caller's contract.
                    unsafe { out.write(library) };
                    true
                }
                Err(_) => false,
            }
        }
    };
}

/// This copy's linked plugin roots, built, as a library exports them. The
/// memory lives for the process (a library is never unloaded).
pub fn exported() -> FfiLibrary {
    let plugins: Vec<FfiPlugin> = inventory::iter::<crate::InstalledPlugin>
        .into_iter()
        .filter_map(|linked| {
            let build = linked.build?;
            let mut builder = PluginBuilder::new(linked.root);
            build(&mut builder);
            let entries: Vec<FfiEntry> = builder
                .entries()
                .map(|(category, fingerprint, value)| FfiEntry {
                    category: FfiStr::of(category),
                    fingerprint,
                    value,
                })
                .collect();
            let entries: &'static [FfiEntry] = Box::leak(entries.into_boxed_slice());
            Some(FfiPlugin {
                descriptor: FfiStr::of(linked.descriptor),
                root: FfiStr::of(linked.root),
                entries: entries.as_ptr(),
                len: entries.len(),
            })
        })
        .collect();
    let plugins: &'static [FfiPlugin] = Box::leak(plugins.into_boxed_slice());
    FfiLibrary {
        abi: LIBRARY_ABI,
        plugins: plugins.as_ptr(),
        len: plugins.len(),
    }
}

/// What importing a library did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// The plugin ids it installed.
    pub plugins: Vec<String>,
    /// Registrations refused, each naming plugin, category and why.
    pub refused: Vec<String>,
}

/// Import a library's plugins ([`exported`] in its copy): their
/// descriptors join the installed set and their registrations the store,
/// each as the host's type of its category when the fingerprints agree.
///
/// # Safety
///
/// `library` is what the library's [`EXPORT_SYMBOL`] wrote, and the library
/// is never unloaded.
pub unsafe fn import(library: &FfiLibrary) -> Result<Imported, String> {
    if library.abi != LIBRARY_ABI {
        return Err(format!(
            "the library's plugin export is layout {} (this engine reads {LIBRARY_ABI})",
            library.abi
        ));
    }
    let mut imported = Imported::default();
    // SAFETY: the caller's contract: the export's arrays live for the process.
    let plugins = unsafe { std::slice::from_raw_parts(library.plugins, library.len) };
    for plugin in plugins {
        // SAFETY: as above.
        let (source, root) = unsafe { (plugin.descriptor.get(), plugin.root.get()) };
        let descriptor = PluginDescriptor::parse(source)?;
        let mut builder = PluginBuilder::new(root);
        // SAFETY: as above.
        let entries = unsafe { std::slice::from_raw_parts(plugin.entries, plugin.len) };
        for entry in entries {
            // SAFETY: as above.
            let category = unsafe { entry.category.get() };
            let importer = inventory::iter::<Importer>
                .into_iter()
                .find(|importer| importer.category == category);
            match importer {
                None => imported.refused.push(format!(
                    "plugin {}: a `{category}` registration, a category this engine does not have",
                    descriptor.id
                )),
                Some(importer) if (importer.fingerprint)() != entry.fingerprint => {
                    imported.refused.push(format!(
                        "plugin {}: its `{category}` registration was built against another \
                         layout of the type (fingerprint {:016x}, this engine's {:016x}); rebuild \
                         the plugin against this engine",
                        descriptor.id,
                        entry.fingerprint,
                        (importer.fingerprint)()
                    ))
                }
                // SAFETY: the fingerprints agree, and the value lives for the
                // process (the caller's contract).
                Some(importer) => unsafe { (importer.import)(&mut builder, entry.value) },
            }
        }
        imported.plugins.push(descriptor.id.clone());
        crate::add_installed(descriptor);
        crate::import_library(builder);
    }
    Ok(imported)
}

/// A value of a registration type, erased for the boundary.
pub(crate) fn erase<T: Any + Send + Sync>(value: &'static T) -> *const () {
    (value as *const T).cast()
}
