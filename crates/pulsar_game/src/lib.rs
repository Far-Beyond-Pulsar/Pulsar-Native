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

pub mod actor;
pub mod archetype;
pub mod component;
pub mod component_store;
pub mod entity;
pub mod event;
pub mod query;
pub mod schedule;
pub mod task;
pub mod tick;
pub mod time;
mod world;

// Blueprint runtime system
pub mod blueprint_runtime;

// Window / rendering integration
pub mod freecam;
pub mod window;
pub mod windowed_app;

// Flatten the most commonly-used types to the crate root.
pub use actor::{Actor, ActorRegistry};
pub use archetype::{Archetype, ArchetypeId, ArchetypeKey};
pub use component::Component;
pub use component_store::{__bp_clear_comp_ctx, __bp_set_comp_ctx, __bp_with_comp, ComponentStore};
pub use entity::Entity;
pub use event::{EventBuffer, EventReader, EventWriter};
pub use query::{QueryIter, WorldQuery};
pub use schedule::Schedule;
pub use task::TaskPool;
pub use tick::{SharedTickLoop, TickLoop, TickMode};
pub use time::GameTime;
pub use window::{RenderCamera, WindowDescriptor, WindowHandle, WindowManager};
pub use world::World;

#[cfg(test)]
mod tests;

/// Convenience prelude — glob-import this to get the whole public API.
pub mod prelude {
    pub use crate::{
        actor::{Actor, ActorRegistry},
        component::Component,
        component_store::ComponentStore,
        entity::Entity,
        event::{EventReader, EventWriter},
        query::{QueryIter, WorldQuery},
        schedule::Schedule,
        task::TaskPool,
        tick::{SharedTickLoop, TickLoop, TickMode},
        time::GameTime,
        window::{RenderCamera, WindowDescriptor, WindowHandle, WindowManager},
        world::World,
    };
}
