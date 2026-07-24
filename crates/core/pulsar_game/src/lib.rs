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
pub use pulsar_scenedb::{
    __bp_clear_comp_ctx, __bp_set_comp_ctx, __bp_with_comp, Actor, ActorRegistry, Archetype,
    ArchetypeId, ArchetypeKey, Component, ComponentStore, Entity, QueryIter, Schedule, World,
    WorldQuery,
};

// Blueprint runtime system
pub mod blueprint_runtime;

// Window / rendering integration
pub mod freecam;
pub mod window;
pub mod windowed_app;

// Play In Editor — host-driven embedding (issue #243)
pub mod embed;

// Legacy tick loop (uses extracted primitives)
pub mod tick;

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
        Actor, ActorRegistry, Component, ComponentStore, Entity, QueryIter, Schedule, World,
        WorldQuery,
    };
}
