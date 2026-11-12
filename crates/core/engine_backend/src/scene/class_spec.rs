//! A plugin's component classes, as its root registers them:
//! `p.components().class::<T>(ClassSpec { requires, systems, actor, gpu,
//! presentation })`.
//!
//! - **requires**: the classes placed with it. Placing the class (the
//!   editor's Add Component, a script's add:
//!   `pulsar_world_registry::place_component`) attaches each one the object
//!   lacks as its own component-instance entity, nested under it. Each must
//!   come from a plugin whose `class` contract the registering plugin
//!   requires; the loader refuses any other
//!   (`pulsar_world_registry::required_class`).
//! - **systems**: per-frame work ordered by labels
//!   (`pulsar_world_registry::systems`), run while the class is registered.
//! - **actor**: its lifecycle events, a capability table of its own
//!   (`pulsar_component_lifecycle`), never reflected methods: not reachable
//!   from Blueprints.
//! - **gpu**: the source rows the class derives and their join
//!   ([`SceneRowsRegistration`]).
//! - **presentation**: its own card, or a facade: one card and one script
//!   component over itself and what it requires
//!   (`pulsar_world_registry::Presentation`).
//!
//! How others do it: Unity's `[RequireComponent(typeof(T))]` adds the
//! required component to the same GameObject when one is added (and keeps
//! it from being removed); Unreal's actor classes create their default
//! subobjects in the constructor (`CreateDefaultSubobject`), and its
//! components tick in tick groups with prerequisites; Bevy 0.15's required
//! components insert defaults on the same entity. Ours attaches the
//! required classes as separate instances (component instances are
//! entities, `pulsar_scene_model::attachments`), so each keeps its own
//! identity, data and plugin: a disabled plugin's member loads unresolved
//! with its data, the rest keep working.

pub use pulsar_component_lifecycle::{
    Actor, ActorCx, ActorTable, ConstructInfo, ConstructReason, SessionKind, SessionScope,
    WorldActor,
};
use pulsar_component_lifecycle::ActorRegistration;
use pulsar_plugins::PluginBuilder;
use pulsar_world_registry::{
    Presentation, System, SystemRegistration, WorldComponentClass, WorldComponentRegistration,
};

/// How a class is registered (see the module docs).
#[derive(Clone, Copy, Default)]
pub struct ClassSpec {
    /// Classes placed with it, by class name.
    pub requires: &'static [&'static str],
    /// Its per-frame systems, ordered by label.
    pub systems: &'static [System],
    /// Its lifecycle events (`begin_play`, `tick`, `end_play`): the
    /// capability registry's table, `ActorTable::of::<T>()`
    /// (`pulsar_component_lifecycle`).
    pub actor: Option<ActorTable>,
    /// The source rows it derives for its plugin's passes.
    #[cfg(feature = "render")]
    pub gpu: Option<ClassRows>,
    pub presentation: Presentation,
}

/// A class's source rows and their join: registered as the plugin's
/// [`SceneRowsRegistration`] named `name`.
#[cfg(feature = "render")]
#[derive(Clone, Copy)]
pub struct ClassRows {
    pub name: &'static str,
    pub register_columns:
        fn(&mut pulsar_scenedb::gpu::SceneGpuStore, &std::sync::Arc<wgpu::Device>),
    pub derivation: Option<
        fn(
            &wgpu::Device,
            helio_scene_join::RowJoinKeys,
        ) -> Box<dyn helio_scene_join::SceneDerivation>,
    >,
}

#[cfg(feature = "render")]
use super::SceneRowsRegistration;

/// The component section of a plugin's builder.
pub struct Components<'b> {
    builder: &'b mut PluginBuilder,
}

impl Components<'_> {
    /// Register class `T` as `spec` says.
    pub fn class<T: WorldComponentClass>(&mut self, spec: ClassSpec) -> &mut Self {
        let origin = self.builder.origin();
        let registration = WorldComponentRegistration {
            origin,
            requires: spec.requires,
            presentation: spec.presentation,
            ..T::REGISTRATION
        };
        let class_name = registration.class_name;
        self.builder.add(registration);
        for system in spec.systems {
            self.builder.add(SystemRegistration {
                origin,
                class_name,
                system: *system,
            });
        }
        if let Some(table) = spec.actor {
            self.builder.add(ActorRegistration {
                origin,
                class_name,
                table,
            });
        }
        #[cfg(feature = "render")]
        if let Some(rows) = spec.gpu {
            self.builder.add(SceneRowsRegistration {
                origin,
                name: rows.name,
                register_columns: rows.register_columns,
                derivation: rows.derivation,
            });
        }
        self
    }
}

/// `p.components()` on a plugin's builder.
pub trait ComponentsBuilder {
    fn components(&mut self) -> Components<'_>;
}

impl ComponentsBuilder for PluginBuilder {
    fn components(&mut self) -> Components<'_> {
        Components { builder: self }
    }
}
