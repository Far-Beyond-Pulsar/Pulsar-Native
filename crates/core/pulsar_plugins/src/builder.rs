//! A plugin's root `build`: the one function through which a plugin
//! registers everything it contributes.
//!
//! ```ignore
//! // fog/src/lib.rs, the root crate.
//! pulsar_plugins::installed!("pulsar-plugin.toml", build);
//!
//! pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
//!     p.add(RenderFeatureRegistration { origin: p.origin(), features, configure });
//!     p.components().class::<GlobalFogComponent>(ClassSpec::default());
//!     #[cfg(feature = "editor")]
//!     editor::build(p);
//! }
//! ```
//!
//! [`crate::installed!`] carries the descriptor and the `build` function
//! into whatever links the root. The first time any registry asks for its
//! registrations ([`crate::registered`]), every linked plugin's `build` runs
//! once, providers before the plugins that require their contracts, into
//! one process-wide store; a plugin library loaded at runtime runs the same
//! `build` across the library boundary and joins the store
//! ([`import_library`]). The engine's own registrations stay link-time
//! (`inventory::submit!` with [`crate::engine_origin!`]): the engine is not
//! a plugin and is always present.
//!
//! Owners add sections as extension traits on the builder
//! (`pulsar_world_registry::ClassBuilder::components`), so this crate knows
//! no registration type. A `build` only adds: it must not read a registry
//! (the store is being filled).
//!
//! How others do it: Bevy's `Plugin::build(&mut App)` is this shape inside
//! one binary; Unreal's `IModuleInterface::StartupModule` registers into
//! global registries from each module, statically linked or loaded as a DLL.
//! Ours is Bevy's single function per plugin with Unreal's descriptor
//! deciding what links, and one store the static and dynamic halves share.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{OnceLock, RwLock};

use crate::{Contribution, PluginDescriptor};

/// What a plugin's `build` registers into: every registration it adds is
/// stamped with the plugin's root as its origin (a registration naming
/// another origin is refused, [`PluginBuilder::problems`]).
pub struct PluginBuilder {
    origin: &'static str,
    added: Vec<Built>,
    problems: Vec<String>,
}

/// One added registration, type-erased.
#[derive(Clone, Copy)]
struct Built {
    type_id: TypeId,
    value: &'static (dyn Any + Send + Sync),
    /// Its category and layout fingerprint, and the value's address: what a
    /// library exports of it (`crate::library`).
    category: &'static str,
    fingerprint: u64,
    raw: *const (),
}

// SAFETY: `raw` is the address of `value`, which is `Send + Sync`.
unsafe impl Send for Built {}
// SAFETY: as above.
unsafe impl Sync for Built {}

impl PluginBuilder {
    /// A builder for the plugin whose root crate is `origin`.
    pub fn new(origin: &'static str) -> Self {
        Self {
            origin,
            added: Vec::new(),
            problems: Vec::new(),
        }
    }

    /// The plugin's root crate: the `origin` every registration it adds
    /// names.
    pub fn origin(&self) -> &'static str {
        self.origin
    }

    /// Register `registration`. It lives for the process (registrations are
    /// read by `&'static` reference, as link-time ones are).
    pub fn add<T: Contribution + Send + Sync + 'static>(&mut self, registration: T) -> &mut Self {
        if !crate::same_package(registration.origin(), self.origin) {
            self.problems.push(format!(
                "`{}` adds a {} registration (`{}`) naming origin `{}`; a plugin's build \
                 registers under its own root",
                self.origin,
                T::CATEGORY,
                registration.id(),
                registration.origin()
            ));
            return self;
        }
        let value: &'static T = Box::leak(Box::new(registration));
        self.added.push(Built::of(value));
        self
    }

    /// What it added: each registration's category, layout fingerprint and
    /// address, as a library exports them.
    pub fn entries(&self) -> impl Iterator<Item = (&'static str, u64, *const ())> + '_ {
        self.added
            .iter()
            .map(|built| (built.category, built.fingerprint, built.raw))
    }

    /// What the build added that was refused.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// How many registrations it added.
    pub fn len(&self) -> usize {
        self.added.len()
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
    }
}

impl Built {
    fn of<T: Contribution + Send + Sync + 'static>(value: &'static T) -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            value,
            category: T::CATEGORY,
            fingerprint: crate::library::fingerprint::<T>(),
            raw: crate::library::erase(value),
        }
    }
}

/// The registrations every linked (and loaded) plugin's build added.
#[derive(Default)]
struct Store {
    by_type: HashMap<TypeId, Vec<&'static (dyn Any + Send + Sync)>>,
    /// Plugin roots whose build ran, in the order they ran.
    built: Vec<&'static str>,
    problems: Vec<String>,
}

impl Store {
    fn absorb(&mut self, builder: PluginBuilder) {
        for built in builder.added {
            self.by_type
                .entry(built.type_id)
                .or_default()
                .push(built.value);
        }
        self.problems.extend(builder.problems);
        self.built.push(builder.origin);
    }
}

fn store() -> &'static RwLock<Store> {
    static STORE: OnceLock<RwLock<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        let mut store = Store::default();
        for (origin, build) in build_order() {
            let mut builder = PluginBuilder::new(origin);
            build(&mut builder);
            store.absorb(builder);
        }
        for problem in &store.problems {
            tracing::error!("{problem}");
        }
        RwLock::new(store)
    })
}

/// Every linked plugin root with a build, providers before the plugins that
/// require their contracts (ties by plugin id).
fn build_order() -> Vec<(&'static str, fn(&mut PluginBuilder))> {
    let mut roots: BTreeMap<
        String,
        (
            &'static PluginDescriptor,
            &'static str,
            fn(&mut PluginBuilder),
        ),
    > = BTreeMap::new();
    for linked in inventory::iter::<crate::InstalledPlugin> {
        let Some(build) = linked.build else { continue };
        if let Some(descriptor) = crate::installed()
            .iter()
            .find(|descriptor| descriptor.owns(linked.root))
        {
            roots.insert(descriptor.id.clone(), (descriptor, linked.root, build));
        }
    }
    let mut order = Vec::new();
    let mut placed: BTreeSet<String> = BTreeSet::new();
    while placed.len() < roots.len() {
        let before = placed.len();
        for (id, (descriptor, root, build)) in &roots {
            if placed.contains(id) {
                continue;
            }
            let waiting = descriptor.requires.iter().any(|required| {
                roots.iter().any(|(other, (provider, ..))| {
                    other != id
                        && !placed.contains(other)
                        && provider.provided(&required.id).is_some()
                })
            });
            if !waiting {
                placed.insert(id.clone());
                order.push((*root, *build));
            }
        }
        if placed.len() == before {
            // A cycle of requirements: the rest by id.
            for (id, (_, root, build)) in &roots {
                if placed.insert(id.clone()) {
                    order.push((*root, *build));
                }
            }
        }
    }
    order
}

/// The registrations of type `T` every plugin build added, not gated: a
/// registry with its own gate (world classes, across plugin libraries)
/// reads these; every other registry reads [`crate::registered`].
pub fn built<T: 'static>() -> Vec<&'static T> {
    let store = store().read().unwrap_or_else(|poison| poison.into_inner());
    store
        .by_type
        .get(&TypeId::of::<T>())
        .into_iter()
        .flatten()
        .filter_map(|value| value.downcast_ref::<T>())
        .collect()
}

/// The plugin roots whose build ran (linked ones, then libraries), and what
/// their builds added that was refused.
pub fn builds() -> (Vec<&'static str>, Vec<String>) {
    let store = store().read().unwrap_or_else(|poison| poison.into_inner());
    (store.built.clone(), store.problems.clone())
}

/// Add a plugin library's registrations, made by its `build` across the
/// library boundary, to the store (the host's plugin loader calls it once
/// per library). `added` are the library's registrations already matched to
/// the host's types (the loader checks each type's layout fingerprint
/// first).
pub fn import_library(builder: PluginBuilder) {
    store()
        .write()
        .unwrap_or_else(|poison| poison.into_inner())
        .absorb(builder);
    crate::bump_generation();
}

/// Add a registration of a host type `T` made by a library's build: the
/// loader has checked that the library's type has `T`'s layout fingerprint,
/// so the reference is read as `T`.
///
/// # Safety
///
/// `value` must point to a value laid out as `T` that lives for the
/// process (the library is never unloaded).
pub unsafe fn add_foreign<T: Contribution + Send + Sync + 'static>(
    builder: &mut PluginBuilder,
    value: *const (),
) {
    // SAFETY: the caller guarantees `value` is a `T` for the process.
    let value: &'static T = unsafe { &*(value as *const T) };
    builder.added.push(Built::of(value));
}
