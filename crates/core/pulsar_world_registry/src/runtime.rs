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
use std::sync::{Mutex, OnceLock};

use pulsar_component_lifecycle::{ActorRegistration, ActorTable};
use pulsar_scenedb::runtime::{AppendList, AttachError};

use crate::systems::SystemRegistration;
use crate::unfinished::UnfinishedComponentRegistration;
use crate::{ComponentEventRegistration, ComponentTickRegistration, WorldComponentRegistration};

/// Version of [`WorldRuntimes`]' and [`Runtime`]'s ABI. Bump it on any
/// change to their fields or to a type passed through them.
pub const ABI_VERSION: u64 = 4;

/// The layouts the fingerprint covers, by name: every type passed through
/// [`WorldRuntimes`] and [`Runtime`].
const LAYOUTS: [(&str, usize); 13] = [
    ("WorldRuntimes", size_of::<WorldRuntimes>()),
    ("Runtime", size_of::<Runtime>()),
    ("Registrations", size_of::<Registrations>()),
    (
        "WorldComponentRegistration",
        size_of::<WorldComponentRegistration>(),
    ),
    (
        "ComponentTickRegistration",
        size_of::<ComponentTickRegistration>(),
    ),
    (
        "ComponentEventRegistration",
        size_of::<ComponentEventRegistration>(),
    ),
    (
        "UnfinishedComponentRegistration",
        size_of::<UnfinishedComponentRegistration>(),
    ),
    ("SystemRegistration", size_of::<SystemRegistration>()),
    ("ActorRegistration", size_of::<ActorRegistration>()),
    ("ActorTable", size_of::<ActorTable>()),
    ("pulsar_reflection", pulsar_reflection::runtime::ABI_VERSION as usize),
    ("pulsar_scenedb", pulsar_scenedb::runtime::ABI_VERSION as usize),
    (
        "pulsar_scene_model",
        pulsar_scene_model::runtime::ABI_VERSION as usize,
    ),
];

const FINGERPRINT: u64 = {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash ^= ABI_VERSION;
    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    let mut index = 0;
    while index < LAYOUTS.len() {
        hash ^= LAYOUTS[index].1 as u64;
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
    systems: fn() -> &'static [&'static SystemRegistration],
    actors: fn() -> &'static [&'static ActorRegistration],
    add_registrations: fn(&Registrations),
}

/// The registrations one copy's `inventory` collected.
pub struct Registrations {
    world_components: Vec<&'static WorldComponentRegistration>,
    ticks: Vec<&'static ComponentTickRegistration>,
    events: Vec<&'static ComponentEventRegistration>,
    unfinished: Vec<&'static UnfinishedComponentRegistration>,
    systems: Vec<&'static SystemRegistration>,
    actors: Vec<&'static ActorRegistration>,
}

impl Registrations {
    fn collected() -> Self {
        Self {
            world_components: world_component_registrations(),
            ticks: inventory::iter::<ComponentTickRegistration>
                .into_iter()
                .collect(),
            events: inventory::iter::<ComponentEventRegistration>
                .into_iter()
                .collect(),
            unfinished: inventory::iter::<UnfinishedComponentRegistration>
                .into_iter()
                .collect(),
            systems: pulsar_plugins::built::<SystemRegistration>(),
            actors: pulsar_plugins::built::<ActorRegistration>(),
        }
    }
}

/// This copy's world classes: the engine's (link-time) and every plugin
/// build's (`ClassSpec`, through the plugin's builder).
fn world_component_registrations() -> Vec<&'static WorldComponentRegistration> {
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .chain(pulsar_plugins::built::<WorldComponentRegistration>())
        .collect()
}

static WORLD_COMPONENTS: AppendList<WorldComponentRegistration> =
    AppendList::new(world_component_registrations);
static SYSTEMS: AppendList<SystemRegistration> =
    AppendList::new(pulsar_plugins::built::<SystemRegistration>);
static ACTORS: AppendList<ActorRegistration> =
    AppendList::new(pulsar_plugins::built::<ActorRegistration>);
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

// The registries hand out only what passes the plugin gate: a class whose
// origin is a disabled plugin's module is not registered, and neither are
// its ticks, events or unfinished-class reports (`pulsar_plugins`). The
// gate runs in the copy that owns the registries, where the project's
// plugins are activated.
static OWN: Runtime = Runtime {
    abi: FINGERPRINT,
    world_components: || {
        static GATED: Gated<WorldComponentRegistration> = Gated::new();
        GATED.get(WORLD_COMPONENTS.get(), |r| {
            pulsar_plugins::origin_enabled(r.origin)
        })
    },
    ticks: || {
        static GATED: Gated<ComponentTickRegistration> = Gated::new();
        GATED.get(TICKS.get(), |r| class_enabled(r.class_name))
    },
    events: || {
        static GATED: Gated<ComponentEventRegistration> = Gated::new();
        GATED.get(EVENTS.get(), |r| class_enabled(r.class_name))
    },
    unfinished: || {
        static GATED: Gated<UnfinishedComponentRegistration> = Gated::new();
        GATED.get(UNFINISHED.get(), |r| class_enabled(r.class_name))
    },
    systems: || {
        static GATED: Gated<SystemRegistration> = Gated::new();
        GATED.get(SYSTEMS.get(), |r| class_enabled(r.class_name))
    },
    actors: || {
        static GATED: Gated<ActorRegistration> = Gated::new();
        GATED.get(ACTORS.get(), |r| class_enabled(r.class_name))
    },
    add_registrations: |r| {
        SYSTEMS.extend(&r.systems);
        ACTORS.extend(&r.actors);
        WORLD_COMPONENTS.extend(&r.world_components);
        TICKS.extend(&r.ticks);
        EVENTS.extend(&r.events);
        UNFINISHED.extend(&r.unfinished);
    },
};

/// Whether world component class `class_name` is not a disabled plugin's.
fn class_enabled(class_name: &str) -> bool {
    WORLD_COMPONENTS
        .get()
        .iter()
        .filter(|r| r.class_name == class_name)
        .all(|r| pulsar_plugins::origin_enabled(r.origin))
}

/// A registry's registrations that pass the plugin gate, rebuilt when the
/// activated plugins or the registrations change (a plugin library appends
/// its own). Each rebuild leaks one list: they happen once per project
/// opened.
struct Gated<T: 'static> {
    cached: Mutex<Option<(u64, usize, &'static [&'static T])>>,
}

impl<T: 'static> Gated<T> {
    const fn new() -> Self {
        Self {
            cached: Mutex::new(None),
        }
    }

    fn get(&self, all: &'static [&'static T], keep: impl Fn(&T) -> bool) -> &'static [&'static T] {
        let generation = pulsar_plugins::generation();
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some((cached_generation, len, list)) = *cached {
            if cached_generation == generation && len == all.len() {
                return list;
            }
        }
        let list: &'static [&'static T] = if all.iter().all(|r| keep(r)) {
            all
        } else {
            Box::leak(
                all.iter()
                    .copied()
                    .filter(|r| keep(r))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            )
        };
        *cached = Some((generation, all.len(), list));
        list
    }
}

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

/// Every world component class this copy collected or was handed, whatever
/// its plugin's state: in the editor's copy (the owner), every class in the
/// process. What the plugin catalog lists; registries use
/// [`world_components`].
pub(crate) fn all_world_components() -> &'static [&'static WorldComponentRegistration] {
    WORLD_COMPONENTS.get()
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

/// Take what plugin libraries' builds added since the lists were made
/// (`pulsar_plugins::library::import`): the plugin loader calls it after
/// importing a library, in the copy that owns the registries.
pub fn import_builds() {
    let new_classes: Vec<&'static WorldComponentRegistration> =
        pulsar_plugins::built::<WorldComponentRegistration>()
            .into_iter()
            .filter(|built| {
                !WORLD_COMPONENTS
                    .get()
                    .iter()
                    .any(|r| std::ptr::eq(*r, *built))
            })
            .collect();
    WORLD_COMPONENTS.extend(&new_classes);
    let new_systems: Vec<&'static SystemRegistration> =
        pulsar_plugins::built::<SystemRegistration>()
            .into_iter()
            .filter(|built| !SYSTEMS.get().iter().any(|r| std::ptr::eq(*r, *built)))
            .collect();
    SYSTEMS.extend(&new_systems);
    let new_actors: Vec<&'static ActorRegistration> =
        pulsar_plugins::built::<ActorRegistration>()
            .into_iter()
            .filter(|built| !ACTORS.get().iter().any(|r| std::ptr::eq(*r, *built)))
            .collect();
    ACTORS.extend(&new_actors);
}

/// Register a lifecycle table in this test binary, as a plugin build would.
#[cfg(test)]
pub(crate) fn register_actor_for_test(registration: &'static ActorRegistration) {
    ACTORS.extend(&[registration]);
}

/// Every lifecycle table the linked plugins register, whatever their state.
pub(crate) fn all_actors() -> &'static [&'static ActorRegistration] {
    ACTORS.get()
}

/// The lifecycle tables of the registered classes
/// ([`pulsar_component_lifecycle`]), gated with their class.
pub(crate) fn actors() -> &'static [&'static ActorRegistration] {
    (runtime().actors)()
}

/// The systems of the registered classes ([`crate::systems`]).
pub(crate) fn systems() -> &'static [&'static SystemRegistration] {
    (runtime().systems)()
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
    // Check every runtime before attaching any: a refusal after the first
    // attach would leave this copy half on its own state and half on the
    // host's (#1083). Each crate's `shared()` is still this copy's own
    // runtime here, or the host's if it is already attached, which then
    // matches.
    let checks = [
        (
            "pulsar_reflection",
            pulsar_reflection::runtime::shared().abi,
            owner.reflection.abi,
        ),
        (
            "pulsar_scenedb",
            pulsar_scenedb::runtime::shared().abi,
            owner.scenedb.abi,
        ),
        (
            "pulsar_scene_model",
            pulsar_scene_model::runtime::shared().abi,
            owner.scene_model.abi,
        ),
        ("pulsar_world_registry", OWN.abi, owner.registry.abi),
    ];
    for (runtime, expected, found) in checks {
        if expected != found {
            return Err(refused(runtime)(AttachError::Abi { expected, found }));
        }
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
            // A panic must not unwind across the FFI boundary (it would
            // abort the editor): the host reads it as a refusal.
            ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                // SAFETY: the caller's contract.
                unsafe { $crate::runtime::attach_exported(host) }
            }))
            .unwrap_or(false)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fingerprint covers the lifecycle registry's types: a plugin
    /// library built against another layout of them is refused.
    #[test]
    fn the_fingerprint_covers_the_lifecycle_types() {
        let layout = |name: &str| LAYOUTS.iter().find(|(n, _)| *n == name).map(|(_, size)| *size);
        assert_eq!(layout("ActorRegistration"), Some(size_of::<ActorRegistration>()));
        assert_eq!(layout("ActorTable"), Some(size_of::<ActorTable>()));
        // `Runtime` carries the table accessor, `Registrations` the list.
        assert!(size_of::<Runtime>() >= 8 * 7);
        assert_eq!(ABI_VERSION, 4);
    }

    #[test]
    fn a_mismatched_runtime_is_refused_before_anything_attaches() {
        let mut scene_model = *pulsar_scene_model::runtime::shared();
        scene_model.abi ^= 1;
        let owner = WorldRuntimes {
            scene_model: Box::leak(Box::new(scene_model)),
            ..*host()
        };
        // SAFETY: `owner` and everything it names live for the process.
        let error = unsafe { attach(&owner) }.unwrap_err();
        assert_eq!(error.runtime, "pulsar_scene_model");
        assert!(matches!(error.error, AttachError::Abi { .. }));

        // The host's own table attaches (to itself) as before.
        // SAFETY: as above.
        unsafe { attach(host()) }.unwrap();
    }
}
