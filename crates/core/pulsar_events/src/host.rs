//! The process-wide host bus, shared with plugins across library
//! boundaries (Pulsar-Native#930).
//!
//! Each dynamic library that links this crate gets its own copy of its
//! statics. So the host (the editor or the game binary) owns the one real
//! bus, a Gamma [`SyncEventBus`], and a plugin loaded as a separate library
//! is handed that bus when it is loaded:
//!
//! 1. The plugin exports [`ATTACH_SYMBOL`] (`export_plugin!` does it with
//!    [`export_host_bus_attach!`](crate::export_host_bus_attach)).
//! 2. The host's plugin loader calls it with [`export_host_bus`], a Gamma
//!    [`RawBus`] (a `#[repr(C)]` table of `extern "C"` functions).
//! 3. The plugin's copy of this crate wraps it in a
//!    [`ForeignBus`](gamma::ffi::ForeignBus); from then on [`host_bus`] in
//!    the plugin is the host's bus, and the asset API
//!    ([`crate::assets`]) publishes and subscribes there.
//!
//! Only `#[repr(C)]` data, Gamma's versioned byte encoding and function
//! pointers cross the boundary, and every handler is dropped by the library
//! that created it (see `gamma::ffi`). A plugin must be attached before it
//! first touches the bus; attaching a copy that already created its own
//! local bus is refused ([`ATTACH_ALREADY_LOCAL`]).
//!
//! This bus carries editor/process-wide events (asset updates). Game
//! events live on each session's [`EventHub`](crate::EventHub), which can be
//! exported to a plugin the same way with
//! [`EventHub::export_raw`](crate::EventHub::export_raw).

use std::sync::OnceLock;

use gamma::ffi::{ForeignBus, ForeignSubscription, RawBus};
use gamma::{DynEvent, EventDescriptor, SubscribeOptions, SyncEventBus, SyncSubscription};

/// The symbol a plugin library exports to receive the host bus:
/// `unsafe extern "C" fn(RawBus) -> u32` ([`AttachFn`]).
pub const ATTACH_SYMBOL: &str = "_plugin_attach_event_bus";

/// Signature of [`ATTACH_SYMBOL`].
pub type AttachFn = unsafe extern "C" fn(RawBus) -> u32;

/// [`ATTACH_SYMBOL`] result: attached.
pub const ATTACH_OK: u32 = 0;
/// [`ATTACH_SYMBOL`] result: this copy already uses a bus (its own, or one
/// attached earlier). The `RawBus` was released.
pub const ATTACH_ALREADY_LOCAL: u32 = 1;
/// [`ATTACH_SYMBOL`] result: the plugin's Gamma speaks another FFI ABI
/// version. The `RawBus` is leaked (its functions cannot be trusted).
pub const ATTACH_ABI_MISMATCH: u32 = 2;

/// A bus: the host's own, or the host's seen from a plugin.
pub enum HostBus {
    Local(SyncEventBus),
    Foreign(ForeignBus),
}

/// A subscription on a [`HostBus`]; unsubscribes on drop.
#[must_use = "dropping the subscription unsubscribes immediately"]
pub enum HostSubscription {
    Local(SyncSubscription),
    Foreign(ForeignSubscription),
}

impl std::fmt::Debug for HostSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(s) => write!(f, "HostSubscription::Local({})", s.token()),
            Self::Foreign(s) => write!(f, "HostSubscription::Foreign({})", s.token()),
        }
    }
}

impl HostSubscription {
    /// Keep the handler for the bus's lifetime.
    pub fn detach(self) {
        match self {
            Self::Local(s) => s.detach(),
            Self::Foreign(s) => s.detach(),
        }
    }
}

impl HostBus {
    /// A new local bus.
    pub fn local() -> Self {
        Self::Local(SyncEventBus::new())
    }

    /// Wrap a bus exported by another library.
    ///
    /// # Safety
    /// See [`ForeignBus::from_raw`]: `raw` must be a fresh export whose
    /// reference is not released elsewhere, and the exporting library must
    /// outlive this bus.
    pub unsafe fn foreign(raw: RawBus) -> Result<Self, gamma::ffi::ForeignError> {
        // SAFETY: forwarded.
        unsafe { ForeignBus::from_raw(raw) }.map(Self::Foreign)
    }

    pub fn is_foreign(&self) -> bool {
        matches!(self, Self::Foreign(_))
    }

    /// Register a descriptor (identical re-registration is a no-op).
    pub fn register_descriptor(&self, descriptor: &EventDescriptor) -> Result<(), String> {
        match self {
            Self::Local(bus) => bus.register_descriptor(descriptor.clone()).map(drop).map_err(|e| e.to_string()),
            Self::Foreign(bus) => bus.register_descriptor(descriptor).map_err(|e| format!("{e:?}")),
        }
    }

    /// Deliver a dynamic event now.
    pub fn publish_dyn(&self, channel: gamma::Channel, event: &DynEvent) -> Result<(), String> {
        match self {
            Self::Local(bus) => bus.publish_dyn(channel, event).map_err(|e| e.to_string()),
            Self::Foreign(bus) => bus.publish_dyn(channel, event).map_err(|e| format!("{e:?}")),
        }
    }

    /// Subscribe to event `id` as a dynamic event.
    pub fn subscribe_dyn(
        &self,
        id: u64,
        opts: SubscribeOptions,
        handler: impl Fn(&DynEvent) + Send + Sync + 'static,
    ) -> HostSubscription {
        match self {
            Self::Local(bus) => HostSubscription::Local(bus.subscribe_dyn(id, opts, handler)),
            Self::Foreign(bus) => HostSubscription::Foreign(bus.subscribe_dyn(id, opts, handler)),
        }
    }

    /// Export for another library. `None` for a foreign bus: a plugin does
    /// not re-export the host's bus.
    pub fn export_raw(&self) -> Option<RawBus> {
        match self {
            Self::Local(bus) => Some(bus.export_raw()),
            Self::Foreign(_) => None,
        }
    }
}

static HOST: OnceLock<HostBus> = OnceLock::new();

/// This library's view of the process-wide bus: the host's own bus in the
/// host, the attached host bus in a plugin. A copy of this crate that was
/// never attached (the host, a plugin compiled into the host, a test)
/// creates a local bus on first use.
pub fn host_bus() -> &'static HostBus {
    HOST.get_or_init(HostBus::local)
}

/// The host bus as a [`RawBus`] to hand to a plugin being loaded. `None`
/// when this copy is itself a plugin.
pub fn export_host_bus() -> Option<RawBus> {
    host_bus().export_raw()
}

/// Make the host's bus this copy's [`host_bus`]. Returns an `ATTACH_*`
/// status.
///
/// # Safety
/// `raw` must come from [`export_host_bus`] (or any Gamma `export_raw`) in
/// the host, and the host must outlive this library.
pub unsafe fn attach_host_bus(raw: RawBus) -> u32 {
    if HOST.get().is_some() {
        // Release the reference we were given; this copy keeps its bus.
        // SAFETY: `raw` is a valid export (caller); releasing it once.
        if let Ok(bus) = unsafe { ForeignBus::from_raw(raw) } {
            drop(bus);
        }
        return ATTACH_ALREADY_LOCAL;
    }
    // SAFETY: forwarded from the caller.
    let bus = match unsafe { HostBus::foreign(raw) } {
        Ok(bus) => bus,
        Err(_) => return ATTACH_ABI_MISMATCH,
    };
    match HOST.set(bus) {
        Ok(()) => ATTACH_OK,
        // Raced with a first use; the losing bus is dropped (released).
        Err(_) => ATTACH_ALREADY_LOCAL,
    }
}

/// Export [`ATTACH_SYMBOL`] from a plugin library, forwarding to
/// [`attach_host_bus`] in the plugin's copy of this crate.
/// `plugin_editor_api::export_plugin!` invokes it for every plugin.
#[macro_export]
macro_rules! export_host_bus_attach {
    () => {
        /// Receive the host's event bus (Pulsar-Native#930). Called by the
        /// host's plugin loader right after loading this library.
        ///
        /// # Safety
        /// `raw` must be a fresh `RawBus` export from the host.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn _plugin_attach_event_bus(raw: $crate::gamma::ffi::RawBus) -> u32 {
            // SAFETY: forwarded from the loader's contract.
            unsafe { $crate::host::attach_host_bus(raw) }
        }
    };
}
