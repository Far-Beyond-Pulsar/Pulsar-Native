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

pub use pulsar_core::{EventBuffer, EventReader, EventWriter, GameTime, TaskPool, TickMode};
// NOTE (#651): the SceneDB baked-store helpers (`__bp_with_comp` /
// `__bp_set_comp_ctx` / `__bp_clear_comp_ctx` / `ComponentStore`) are
// deliberately NOT re-exported anymore — generated actors address the live
// world through `pulsar_world_registry`'s dispatcher. They remain available
// from `pulsar_scenedb` itself for legacy callers.
pub use pulsar_scenedb::{
    Actor, ActorRegistry, Archetype, ArchetypeId, ArchetypeKey, Component, Entity, QueryIter,
    Schedule, World, WorldQuery,
};

// Blueprint runtime system
pub mod blueprint_runtime;

// Window / rendering integration
pub mod camera_selection;
pub mod freecam;
pub mod window;
pub mod windowed_app;

// Play In Editor — host-driven embedding (issue #243)
pub mod embed;

// Legacy tick loop (uses extracted primitives)
pub mod tick;

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
    pub use helio_component::components::{LightComponent, MeshAssetPath, StaticMeshComponent};
}

// Compile-time drift guard: PBGC-generated actors must match pinned crates
#[cfg(test)]
mod blueprint_codegen_drift;

// #651 acceptance probe: generated-actor shapes mutate the live world
#[cfg(test)]
mod blueprint_live_dispatch;

// #654 acceptance: cross-object reference graphs behave identically through
// the sourcegen path (generated-shape twin + emission drift guard)
#[cfg(test)]
mod blueprint_ref_codegen;

#[cfg(test)]
mod tests;

/// Convenience prelude — glob-import this to get the whole public API.
pub mod prelude {
    pub use crate::{
        blueprint_runtime::{
            BlueprintDispatcher, BlueprintEvent, BlueprintExecutionMode, BlueprintExecutor,
            BlueprintInstance, ByteArena, BytecodeCompiler, CompiledBytecode, ExecutionMode,
            VariableDescriptor,
        },
        freecam::FreeCam,
        tick::{SharedTickLoop, TickLoop},
        window::{RenderCamera, WindowDescriptor, WindowHandle, WindowManager},
    };
    pub use pulsar_core::{EventReader, EventWriter, GameTime, TaskPool, TickMode};
    pub use pulsar_scenedb::{
        Actor, ActorRegistry, Component, Entity, QueryIter, Schedule, World, WorldQuery,
    };
}
