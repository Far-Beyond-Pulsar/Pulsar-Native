//! `WorldComponentRegistration`: one component class's `World` bridge, how a class is submitted, and its catalog source.

use pulsar_reflection::EngineClass;
use pulsar_scenedb::{ComponentId, Entity, World};
use serde_json::Value;

use crate::{runtime, systems, EngineClassMut};

/// Register component classes at link time, for a plugin crate's own unit
/// tests only: a test binary of a component crate links no plugin root, so
/// nothing runs its build. A plugin registers its classes from its root's
/// build (`p.components().class::<T>(spec)`); `plugins/installed`'s
/// `installed_set` test fails on this macro outside `#[cfg(test)]`.
#[macro_export]
macro_rules! submit_world_component {
    ($($ty:ty),* $(,)?) => {$(
        $crate::inventory::submit! {
            $crate::WorldComponentRegistration {
                origin: ::core::env!("CARGO_PKG_NAME"),
                ..<$ty>::PULSAR_WORLD_COMPONENT
            }
        }
    )*};
}

/// One registered component class's `World` bridge. Populated by
/// `#[register_world_component]` or `#[world_component]` plus
/// [`submit_world_component!`] (`engine_class_derive`) -- not constructed
/// by hand.
pub struct WorldComponentRegistration {
    pub class_name: &'static str,
    /// Who registers the class: the engine (`engine:<package>`, where
    /// `#[register_world_component]` expands), or the plugin root that
    /// invokes [`submit_world_component!`]. While that package's plugin is
    /// disabled the class is not registered (`pulsar_plugins`), and its
    /// stored instances load as `UnresolvedComponent`s, kept verbatim.
    pub origin: &'static str,
    /// This class's `pulsar_scenedb::ComponentId`, as a function pointer
    /// rather than a precomputed value -- `component_id::<T>()` isn't
    /// `const fn` (it allocates on a type's first-ever call, via a global
    /// registry), and `inventory::submit!`'s payload has to be const-
    /// evaluable, so `pulsar_scenedb::component_id::<#self_ty>` (the
    /// function item itself, not a call) is what
    /// `#[register_world_component]` actually emits here. Called at lookup
    /// time instead ([`crate::find_by_component_id`]) -- cheap, `component_id::<T>`
    /// caches its own result after the first call regardless of caller.
    ///
    /// This is the identity SceneDB's change journals and object
    /// subscriptions report (`Entity` + `ComponentId`, no notion of a class
    /// name at that layer); [`crate::find_by_component_id`] maps it back to this
    /// registration.
    pub component_type: fn() -> ComponentId,
    /// Construct this class's default value, owned and type-erased -- the
    /// generic factory.
    pub default_value: fn() -> Box<dyn std::any::Any + Send + Sync>,
    /// Decode this class's value from its JSON representation. A boundary
    /// codec (files, external tools): JSON is decoded once, here, and the
    /// result is an owned typed value. Classes with a legacy file shape or
    /// asset data to load supply their own (`decode = path`).
    pub decode: fn(&Value) -> Result<Box<dyn std::any::Any + Send + Sync>, String>,
    /// Clone a value of this class (`None` if `value` is not this class).
    pub clone_value: fn(&dyn std::any::Any) -> Option<Box<dyn std::any::Any + Send + Sync>>,
    /// View an owned value of this class (one not in a `World`, e.g. a
    /// class template's) as `&dyn EngineClass`, for reflected reads.
    pub value_as_engine_class: fn(&dyn std::any::Any) -> Option<&dyn EngineClass>,
    /// [`Self::value_as_engine_class`], mutably, for reflected writes.
    pub value_as_engine_class_mut: fn(&mut dyn std::any::Any) -> Option<&mut dyn EngineClass>,
    /// Register this class with SceneDB for type-erased insertion
    /// (`pulsar_scenedb::register_component::<T>`); idempotent.
    pub register_erased: fn() -> ComponentId,
    /// Remove this class's typed component from `entity`, if present.
    /// Called when the component is deleted, disabled, or its owning object
    /// is despawned.
    pub remove: fn(&mut World, Entity),
    /// Borrow the typed value already in `World` as `&dyn EngineClass` --
    /// the properties panel's *read* path. No JSON, no throwaway `Default`
    /// instance: this is the one real, live value.
    pub get_as_engine_class: fn(&World, Entity) -> Option<&dyn EngineClass>,
    /// Borrow the typed value already in `World` as `&mut dyn EngineClass`
    /// -- the properties panel's *write* path, as an [`EngineClassMut`] guard.
    /// Apply a `PropertyMetadata` setter to `&mut *guard`; SceneDB's write
    /// hooks (GPU mirror, change tracker, journals, object subscriptions) fire when
    /// the guard drops, after the edit.
    pub get_as_engine_class_mut: for<'w> fn(&'w mut World, Entity) -> Option<EngineClassMut<'w>>,
    /// Re-establish data this class derives from its own fields after a
    /// reflected write: `Some(name)` after a property setter, `None` after a
    /// reflected method (which may have written anything). Runs inside the
    /// same write guard as the write itself, so SceneDB observes one commit
    /// with the final value. A component-owned normalization -- e.g.
    /// `StaticMeshComponent` loading the asset its `mesh_asset` now names --
    /// never a GPU refresh: GPU rows follow the write through SceneDB's own
    /// mirror dispatch. Defaults to a no-op (`property_written = path`
    /// overrides it with a `fn(&mut Self, Option<&str>)`).
    pub property_written: fn(&mut dyn EngineClass, Option<&str>),
    /// The classes placed with this one (`ClassSpec::requires`): attaching
    /// it attaches each one the object lacks, as its own component-instance
    /// entity nested under it ([`crate::instances::place_component`]). Each must
    /// come from a plugin whose class contract the registering plugin
    /// requires ([`crate::required_class`]).
    pub requires: &'static [&'static str],
    /// How the editor and scripts present it ([`Presentation`]).
    pub presentation: Presentation,
}

/// How a class shows in the inspector, the component tree and to scripts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presentation {
    /// Its own card and its own entry.
    #[default]
    Component,
    /// One card and one entry over itself and the classes it requires
    /// (attached nested under it): a character that is one component to the
    /// user and to scripts, built from a skeletal mesh, a controller and an
    /// input binding. The members stay real components; the facade's
    /// methods are the script surface.
    Facade,
}

/// A world component class (`#[world_component]` implements it): its
/// registration, which the plugin's root adds through its builder
/// (`p.components().class::<T>(spec)`).
pub trait WorldComponentClass {
    const REGISTRATION: WorldComponentRegistration;
}

inventory::collect!(WorldComponentRegistration);

// A plugin library's classes and systems are imported as these types
// (`pulsar_plugins::library`).
pulsar_plugins::importer!(WorldComponentRegistration, systems::SystemRegistration);

impl pulsar_plugins::Contribution for WorldComponentRegistration {
    const CATEGORY: &'static str = "component.class";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.class_name.to_string()
    }
}

/// World component classes have their own gate (the registry hands out only
/// classes whose plugin is enabled, across plugin libraries: `runtime`), so
/// their catalog source reads the registry instead of
/// `pulsar_plugins::registered`.
fn all_class_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    runtime::all_world_components()
        .iter()
        .map(|registration| registration.summary())
        .collect()
}

/// The classes registered at link time (`#[register_world_component]`, the
/// engine's; a plugin's unit-test registration).
fn linked_class_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .map(|registration| registration.summary())
        .collect()
}

fn active_class_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    runtime::world_components()
        .iter()
        .map(|registration| registration.summary())
        .collect()
}

inventory::submit! {
    pulsar_plugins::ContributionSource {
        category: "component.class",
        all: all_class_summaries,
        active: active_class_summaries,
        linked: linked_class_summaries,
    }
}
