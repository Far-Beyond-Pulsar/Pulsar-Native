//! One view of the plugins per process, across plugin libraries.
//!
//! A plugin library links its own copy of this crate, so it has its own
//! store of registrations, its own installed set and its own activated set:
//! code of the plugin that asks what is registered (`registered::<T>()` in
//! `helio_component`, linked into the library) saw only that library's own
//! registrations, never the other plugins' or the project's enabled set. The
//! host hands every library its view ([`HostPlugins`], through the
//! [`ATTACH_SYMBOL`] entry point `export_library!` generates) before the
//! library runs any code of its own, and the library's readers
//! (`all_registered`, `installed`, `origin_enabled`, `generation`,
//! `is_enabled`) answer from it. Registrations cross by category and layout
//! fingerprint, as a library's own do (`library::fingerprint`), never by
//! type identity.
//!
//! A library's `build` still runs against its own store (the library
//! exports what it added, `library::exported`); the host's store, which
//! absorbs the export, is what the others read.

use std::sync::atomic::{AtomicPtr, Ordering};

use crate::{PluginDescriptor, Registration};

/// Version of [`HostPlugins`]; [`attach`] refuses another.
pub const ABI_VERSION: u64 = 1;

/// The symbol `export_library!` exports for [`attach_exported`].
pub const ATTACH_SYMBOL: &str = "_plugin_attach_plugins";

/// Its signature: attach to the host's [`HostPlugins`], `true` on success.
pub type AttachFn = unsafe extern "C" fn(host: *const std::ffi::c_void) -> bool;

/// The host's view of the plugins, as functions of the copy that owns it.
#[derive(Clone, Copy)]
pub struct HostPlugins {
    pub abi: u64,
    /// The registrations plugin builds added of a category and layout
    /// fingerprint, each handed to the sink as an address.
    pub built: fn(category: &str, fingerprint: u64, sink: &mut dyn FnMut(*const ())),
    pub origin_enabled: fn(origin: &str) -> bool,
    pub is_enabled: fn(plugin: &str) -> bool,
    pub generation: fn() -> u64,
    pub installed: fn() -> &'static [PluginDescriptor],
}

static OWN: HostPlugins = HostPlugins {
    abi: ABI_VERSION,
    built: crate::builder::built_raw,
    origin_enabled: crate::local_origin_enabled,
    is_enabled: crate::local_is_enabled,
    generation: crate::local_generation,
    installed: crate::local_installed,
};

static ATTACHED: AtomicPtr<HostPlugins> = AtomicPtr::new(std::ptr::null_mut());

/// The host's view this copy answers from, once attached.
pub(crate) fn attached() -> Option<&'static HostPlugins> {
    let host = ATTACHED.load(Ordering::Acquire);
    // SAFETY: `attach` stores only a pointer to a table that lives for the
    // process.
    unsafe { host.as_ref() }
}

/// This copy's own view, to hand a plugin library: the one it is attached
/// to, if it is itself attached.
pub fn host() -> &'static HostPlugins {
    attached().unwrap_or(&OWN)
}

/// Why [`attach`] refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachError {
    Null,
    Abi { expected: u64, found: u64 },
    AlreadyAttached,
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => write!(f, "the host's plugin view is null"),
            Self::Abi { expected, found } => write!(
                f,
                "the host's plugin view is layout {found} (this copy reads {expected})"
            ),
            Self::AlreadyAttached => write!(f, "attached to another host's plugin view"),
        }
    }
}

impl std::error::Error for AttachError {}

/// Answer from `host` from now on. Attaching the same table again is fine.
///
/// # Safety
///
/// `host` is null or points to a [`HostPlugins`] that lives for the rest of
/// the process, as do its functions.
pub unsafe fn attach(host: *const HostPlugins) -> Result<(), AttachError> {
    // SAFETY: the caller's contract.
    let host = unsafe { host.as_ref() }.ok_or(AttachError::Null)?;
    if host.abi != ABI_VERSION {
        return Err(AttachError::Abi {
            expected: ABI_VERSION,
            found: host.abi,
        });
    }
    if std::ptr::eq(host, &OWN) {
        return Ok(());
    }
    let pointer = host as *const HostPlugins as *mut HostPlugins;
    match ATTACHED.compare_exchange(
        std::ptr::null_mut(),
        pointer,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Ok(()),
        Err(current) if current == pointer => Ok(()),
        Err(_) => Err(AttachError::AlreadyAttached),
    }
}

/// [`attach`] for [`ATTACH_SYMBOL`]: logs a refusal and reports it as `false`.
///
/// # Safety
///
/// As [`attach`].
#[doc(hidden)]
pub unsafe fn attach_exported(host: *const std::ffi::c_void) -> bool {
    // SAFETY: the caller's contract.
    match unsafe { attach(host.cast()) } {
        Ok(()) => true,
        Err(error) => {
            tracing::error!("refused the host's plugin view: {error}");
            false
        }
    }
}

/// The registrations of `T` the attached host's builds hold; `None` when
/// this copy is not attached (its own store is the whole one).
pub(crate) fn host_built<T: Registration>() -> Option<Vec<&'static T>> {
    let host = attached()?;
    let mut found = Vec::new();
    (host.built)(
        T::CATEGORY,
        crate::library::fingerprint::<T>(),
        &mut |raw| {
            // SAFETY: the fingerprints agree (the same check a library's own
            // registrations pass), and the value lives for the process.
            found.push(unsafe { &*(raw as *const T) });
        },
    );
    Some(found)
}
