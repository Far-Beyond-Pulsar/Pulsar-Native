//! The world crates' process-wide state, shared by every statically linked
//! copy (Pulsar-Native#1083).
//!
//! The editor links `pulsar_reflection`, `pulsar_scenedb`,
//! `pulsar_scene_model` and this crate statically, and so does every plugin library it loads. Each crate's
//! process-wide state (component ids, counters, registries) is owned by the
//! editor's copy and reached by every other copy through that crate's
//! runtime, once the copy is attached; see `pulsar_scenedb::runtime` for the
//! model, which is WGPUI's for gpui. [`WorldRuntimes`] gathers the four
//! runtimes so a plugin attaches its copies with one call: the editor hands
//! out [`host`], a plugin calls [`attach`] with it before anything else.
//!
//! This crate's own part is its registries: world component classes,
//! component tick and event registrations, and unfinished-class reports.

use std::fmt;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

use pulsar_scenedb::runtime::{AppendList, AttachError};

use crate::unfinished::UnfinishedComponentRegistration;
use crate::{ComponentEventRegistration, ComponentTickRegistration, WorldComponentRegistration};

/// Version of [`WorldRuntimes`]' and [`Runtime`]'s ABI. Bump it on any
/// change to their fields or to a type passed through them.
pub const ABI_VERSION: u64 = 1;

const FINGERPRINT: u64 = {
    let parts = [
        ABI_VERSION as usize,
        size_of::<WorldRuntimes>(),
        size_of::<Runtime>(),
        size_of::<Registrations>(),
        size_of::<WorldComponentRegistration>(),
        size_of::<ComponentTickRegistration>(),
        size_of::<ComponentEventRegistration>(),
        size_of::<UnfinishedComponentRegistration>(),
        pulsar_reflection::runtime::ABI_VERSION as usize,
        pulsar_scenedb::runtime::ABI_VERSION as usize,
        pulsar_scene_model::runtime::ABI_VERSION as usize,
    ];
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < parts.len() {
        hash ^= parts[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
};

/// The runtimes of the world crates, as the editor hands them to a plugin.
/// Each crate's runtime carries its own ABI check too.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WorldRuntimes {
    /// [`ABI_VERSION`] and the layout fingerprint of the copy that built
    /// this table; [`attach`] refuses a table whose `abi` differs.
    pub abi: u64,
    pub reflection: &'static pulsar_reflection::runtime::Runtime,
    pub scenedb: &'static pulsar_scenedb::runtime::Runtime,
    pub scene_model: &'static pulsar_scene_model::runtime::Runtime,
    pub registry: &'static Runtime,
}

/// This crate's registries, as the functions of the copy that owns them.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Runtime {
    pub abi: u64,
    world_components: fn() -> &'static [&'static WorldComponentRegistration],
    ticks: fn() -> &'static [&'static ComponentTickRegistration],
    events: fn() -> &'static [&'static ComponentEventRegistration],
    unfinished: fn() -> &'static [&'static UnfinishedComponentRegistration],
    add_registrations: fn(&Registrations),
}

/// The registrations one copy's `inventory` collected.
pub struct Registrations {
    world_components: Vec<&'static WorldComponentRegistration>,
    ticks: Vec<&'static ComponentTickRegistration>,
    events: Vec<&'static ComponentEventRegistration>,
    unfinished: Vec<&'static UnfinishedComponentRegistration>,
}

impl Registrations {
    fn collected() -> Self {
        Self {
            world_components: inventory::iter::<WorldComponentRegistration>
                .into_iter()
                .collect(),
            ticks: inventory::iter::<ComponentTickRegistration>
                .into_iter()
                .collect(),
            events: inventory::iter::<ComponentEventRegistration>
                .into_iter()
                .collect(),
            unfinished: inventory::iter::<UnfinishedComponentRegistration>
                .into_iter()
                .collect(),
        }
    }
}

static WORLD_COMPONENTS: AppendList<WorldComponentRegistration> = AppendList::new(|| {
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .collect()
});
static TICKS: AppendList<ComponentTickRegistration> = AppendList::new(|| {
    inventory::iter::<ComponentTickRegistration>
        .into_iter()
        .collect()
});
static EVENTS: AppendList<ComponentEventRegistration> = AppendList::new(|| {
    inventory::iter::<ComponentEventRegistration>
        .into_iter()
        .collect()
});
static UNFINISHED: AppendList<UnfinishedComponentRegistration> = AppendList::new(|| {
    inventory::iter::<UnfinishedComponentRegistration>
        .into_iter()
        .collect()
});

static OWN: Runtime = Runtime {
    abi: FINGERPRINT,
    world_components: || WORLD_COMPONENTS.get(),
    ticks: || TICKS.get(),
    events: || EVENTS.get(),
    unfinished: || UNFINISHED.get(),
    add_registrations: |r| {
        WORLD_COMPONENTS.extend(&r.world_components);
        TICKS.extend(&r.ticks);
        EVENTS.extend(&r.events);
        UNFINISHED.extend(&r.unfinished);
    },
};

static ATTACHED: AtomicPtr<Runtime> = AtomicPtr::new(std::ptr::null_mut());

#[inline]
fn runtime() -> &'static Runtime {
    let attached = ATTACHED.load(Ordering::Acquire);
    if attached.is_null() {
        &OWN
    } else {
        // SAFETY: `attach` only stores a pointer to a `Runtime` that lives
        // for the process, checked for this ABI.
        unsafe { &*attached }
    }
}

/// Every registered world component class: this copy's and every attached
/// copy's.
pub(crate) fn world_components() -> &'static [&'static WorldComponentRegistration] {
    (runtime().world_components)()
}

pub(crate) fn ticks() -> &'static [&'static ComponentTickRegistration] {
    (runtime().ticks)()
}

pub(crate) fn events() -> &'static [&'static ComponentEventRegistration] {
    (runtime().events)()
}

pub(crate) fn unfinished() -> &'static [&'static UnfinishedComponentRegistration] {
    (runtime().unfinished)()
}

/// The world runtimes this copy uses, to hand a plugin library: its own,
/// or the ones it is attached to.
pub fn host() -> &'static WorldRuntimes {
    static HOST: OnceLock<WorldRuntimes> = OnceLock::new();
    HOST.get_or_init(|| WorldRuntimes {
        abi: FINGERPRINT,
        reflection: pulsar_reflection::runtime::shared(),
        scenedb: pulsar_scenedb::runtime::shared(),
        scene_model: pulsar_scene_model::runtime::shared(),
        registry: runtime(),
    })
}

/// Why [`attach`] refused the world runtimes, and which crate's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldAttachError {
    /// The crate whose runtime was refused (`"world runtimes"` for the
    /// table itself).
    pub runtime: &'static str,
    pub error: AttachError,
}

impl fmt::Display for WorldAttachError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.runtime, self.error)
    }
}

impl std::error::Error for WorldAttachError {}

/// Attach this copy of the world crates to the editor's world runtimes
/// (`owner`, the editor's [`host`]) and hand them the registrations this
/// copy collected. A plugin library calls it once, before anything else.
///
/// # Safety
///
/// `owner` is null or points to a [`WorldRuntimes`] that lives for the rest
/// of the process, as do the runtimes it names and their functions.
pub unsafe fn attach(owner: *const WorldRuntimes) -> Result<(), WorldAttachError> {
    let refused = |runtime| move |error| WorldAttachError { runtime, error };
    // SAFETY: the caller's contract.
    let owner = unsafe { owner.as_ref() }
        .ok_or(AttachError::Null)
        .map_err(refused("world runtimes"))?;
    if owner.abi != FINGERPRINT {
        return Err(refused("world runtimes")(AttachError::Abi {
            expected: FINGERPRINT,
            found: owner.abi,
        }));
    }
    // Reflection first: SceneDB's method table is built from its registries.
    // SAFETY: the caller's contract covers the runtimes `owner` names.
    unsafe { pulsar_reflection::runtime::attach(owner.reflection) }
        .map_err(|error| match error {
            pulsar_reflection::runtime::AttachError::Null => AttachError::Null,
            pulsar_reflection::runtime::AttachError::Abi { expected, found } => {
                AttachError::Abi { expected, found }
            }
            pulsar_reflection::runtime::AttachError::AlreadyAttached => {
                AttachError::AlreadyAttached
            }
        })
        .map_err(refused("pulsar_reflection"))?;
    // Then SceneDB: the other crates' registrations resolve component ids.
    // SAFETY: as above.
    unsafe { pulsar_scenedb::runtime::attach(owner.scenedb) }.map_err(refused("pulsar_scenedb"))?;
    // SAFETY: as above.
    unsafe { pulsar_scene_model::runtime::attach(owner.scene_model) }
        .map_err(refused("pulsar_scene_model"))?;
    let registry = owner.registry;
    if registry.abi != OWN.abi {
        return Err(refused("pulsar_world_registry")(AttachError::Abi {
            expected: OWN.abi,
            found: registry.abi,
        }));
    }
    if std::ptr::eq(registry, &OWN) {
        return Ok(());
    }
    let registry_ptr = registry as *const Runtime as *mut Runtime;
    match ATTACHED.compare_exchange(
        std::ptr::null_mut(),
        registry_ptr,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => {}
        Err(current) if current == registry_ptr => return Ok(()),
        Err(_) => {
            return Err(refused("pulsar_world_registry")(
                AttachError::AlreadyAttached,
            ))
        }
    }
    (registry.add_registrations)(&Registrations::collected());
    Ok(())
}

/// The entry point a plugin library exports for [`attach`]
/// ([`export_world_runtime_attach!`](crate::export_world_runtime_attach)).
pub const ATTACH_SYMBOL: &str = "_plugin_attach_world_runtime";

/// The signature of [`ATTACH_SYMBOL`]: attach to the host's
/// [`WorldRuntimes`], `true` on success.
pub type AttachFn = unsafe extern "C" fn(host: *const std::ffi::c_void) -> bool;

/// [`attach`] for [`ATTACH_SYMBOL`]: logs a refusal and reports it as
/// `false`.
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
            tracing::error!("refused the host's world runtimes: {error}");
            false
        }
    }
}

/// Export [`ATTACH_SYMBOL`] from a plugin library, so the editor's plugin
/// manager attaches the library's world crates to the editor's before the
/// plugin runs any code. `plugin_editor_api::export_plugin!` invokes it.
#[macro_export]
macro_rules! export_world_runtime_attach {
    () => {
        /// Attach this library's world crates to the host's world runtimes
        /// (Pulsar-Native#1083). Called by the plugin manager before
        /// `_plugin_create`.
        ///
        /// # Safety
        ///
        /// `host` is the host's `WorldRuntimes`, alive for the process.
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_attach_world_runtime(
            host: *const ::std::ffi::c_void,
        ) -> bool {
            // SAFETY: the caller's contract.
            unsafe { $crate::runtime::attach_exported(host) }
        }
    };
}
