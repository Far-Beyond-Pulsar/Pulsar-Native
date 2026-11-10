//! # pulsar_game
//!
//! The Pulsar game runtime crate.  Provides:
//!
//! - **Archetypal ECS** — `World`, `Entity`, `Component`, queries.
//! - **Actor lifecycle** — `Actor` trait with `begin_play` / `end_play` / `tick`.
//! - **System schedule** — ordered `Schedule` of ECS `SystemFn`s.
//! - **Tick loop** — `TickLoop` with fixed or variable timestep.
//! - **Async task pool** — smol-backed `TaskPool` for background work.
//! - **Event channels** — `EventWriter<T>` / `EventReader<T>`.
//! - **Game time** — `GameTime` and `DeltaTime`.

#![deny(clippy::undocumented_unsafe_blocks)]

pub use pulsar_core::{EventBuffer, EventReader, EventWriter, GameTime, TaskPool, TickMode};

// Generated actors address the live world through `pulsar_world_registry`'s
// dispatcher.
pub use pulsar_scenedb::{
    Actor, ActorRegistry, Archetype, ArchetypeId, ArchetypeKey, Component, Entity, QueryIter,
    Schedule, World, WorldQuery,
};

// Script classes on the engine script VM (module.json builds).
pub mod scripting;

// Window / rendering integration
pub mod camera_selection;
pub mod freecam;
pub mod window;
pub mod windowed_app;

// Play In Editor — host-driven embedding (issue #243)
pub mod embed;

// Standalone entry point: content discovery, headless runs (#926)
#[cfg(feature = "alloc-profile")]
pub mod alloc_profile;
pub mod standalone;

// Legacy tick loop (uses extracted primitives)
pub mod tick;

// The Blueprints a level assigns to the engine roles, resolved for the spawn step.
pub mod roles;

// Player input through possession: the input component on the pawn, and the
// gate that keeps the free camera off the pawn's keys.
pub mod player_input;

// The entity the local player's view follows (set by possession, read by the
// hosts' camera selection).
pub mod view_target;

// The one pulsar_core::GameTime <-> pulsar_scenedb::GameTime seam (#652)
pub mod time;

// Gameplay script-crate actors: identity-tagged registration + native hot
// reload (#653). Generated projects and scripts/ crates register through
// `TickLoop::register_actor`.
pub mod scripts;

// Cross-object reference resolution for blueprint graphs (#654): the one
// implementation BOTH compile targets call (VM trampolines + generated Rust).
pub mod script_refs;

/// Component vocabulary gameplay scripts attach most often (#653).
///
/// User script crates depend only on `pulsar_game`; this module is their
/// single import surface for making entities visible — a mesh plus its
/// placement. Re-exports, not wrappers: these are the SAME types the
/// renderer's maintainers subscribe to, so mutations made through them are
/// visible on the next frame exactly like editor edits.
pub mod scene {
    pub use engine_backend::scene::{Name, Transform, Visibility};
    /// Components attach to objects as instance entities (Pulsar-Native#1035):
    /// `attach_value` adds one, `attachments` finds and edits them.
    pub use pulsar_world_registry::pulsar_scene_model::attachments;
    pub use pulsar_world_registry::{attach_value, AttachError};

    /// Attach a component of the class `class_name` to `owner` and set its
    /// reflected `properties` by name (each value as JSON text), as the
    /// editor's property edits do: the path every kind takes from a script
    /// that does not link the kind's crate (a mesh:
    /// `attach_class(world, cube, "StaticMeshComponent",
    /// &[("mesh_asset", "\"meshes/primitives/SM_Cube.fbx\"")])`).
    /// Refused when no enabled plugin registers the class or a property does
    /// not take its value; the instance is removed again then.
    pub fn attach_class(
        world: &mut pulsar_scenedb::World,
        owner: pulsar_scenedb::Entity,
        class_name: &str,
        properties: &[(&str, &str)],
    ) -> Result<pulsar_scenedb::Entity, String> {
        // The new instance is the last of its class on the owner.
        let index = pulsar_world_registry::component_records(world, owner)
            .iter()
            .filter(|record| record.class_name == class_name)
            .count() as u32;
        let instance = pulsar_world_registry::attach_component(
            world,
            owner,
            pulsar_world_registry::pulsar_scene_model::NewInstance::new(class_name),
            pulsar_world_registry::ComponentPayload::Default,
        )
        .map_err(|error| error.to_string())?;
        for (property, value) in properties {
            let set = serde_json::from_str(value)
                .map_err(|error| format!("{class_name}.{property}: invalid value: {error}"))
                .and_then(|value| {
                    pulsar_world_registry::set_component_property(
                        world, owner, class_name, index, property, value,
                    )
                    .map_err(|error| error.to_string())
                });
            if let Err(error) = set {
                world.despawn(instance);
                return Err(error);
            }
        }
        Ok(instance)
    }

    /// Whether `owner` has an enabled component of the class `class_name`.
    pub fn has_component(
        world: &pulsar_scenedb::World,
        owner: pulsar_scenedb::Entity,
        class_name: &str,
    ) -> bool {
        pulsar_world_registry::component_records(world, owner)
            .iter()
            .any(|record| record.enabled && record.class_name == class_name)
    }
}

// Exported (generated) actors name this crate as `pulsar_game`; inside it,
// that path is this crate.
extern crate self as pulsar_game;

// No object kind's plugin is linked here: a game built for a project links
// the project's enabled plugins' root crates
// (`engine_backend::services::core_project_builder`), and their components,
// scene subsystems, payload publishers, render features and settings register
// through `inventory`.

// Compile-time drift guard: exported actors must match the pinned crates
#[cfg(test)]
mod blueprint_codegen_drift;

// The always-compiled probe: an actor generated by `build.rs`
#[cfg(test)]
mod export_probe;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_probe;

/// Convenience prelude — glob-import this to get the whole public API.
pub mod prelude {
    pub use crate::{
        freecam::FreeCam,
        scripting::ScriptRuntime,
        tick::{ScriptStats, SharedTickLoop, TickLoop},
        window::{RenderCamera, WindowDescriptor, WindowHandle, WindowManager},
    };
    pub use pulsar_core::{EventReader, EventWriter, GameTime, TaskPool, TickMode};
    pub use pulsar_scenedb::{
        Actor, ActorRegistry, Component, Entity, QueryIter, Schedule, World, WorldQuery,
    };
}
