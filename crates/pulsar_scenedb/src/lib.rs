//! Archetype-based Entity Component System for the Pulsar engine.
//!
//! # Design
//!
//! - **Dense `u32` ComponentId** â€” each component type is assigned a dense ID on
//!   first access.  Columns are stored in a `Vec` indexed by this ID, so hot-path
//!   component lookups require no hashing.
//! - **Archetype bitmask** â€” each archetype stores a `u64` bitmask of its
//!   component types.  Queries check the bitmask before touching any column,
//!   which skips non-matching archetypes in constant time.
//! - **`swap_remove` slot reuse** â€” entity removal swaps the last entity into
//!   the vacated slot.  No tombstones, no compaction passes.
//! - **Thread-local CID cache** â€” `component_id::<T>()` caches its result per
//!   thread, avoiding atomic or mutex operations on the hot query path.
//!
//! # Safety
//!
//! All `unsafe` blocks are accompanied by `// SAFETY:` comments explaining the
//! invariants.  The test suite includes adversarial tests (interleaved mutation,
//! dangling entity rejection, component churn) in addition to correctness tests.
//!
//! # Quick start
//!
//! ```
//! use pulsar_scenedb::{World, Component, QueryIter, WorldQuery};
//!
//! struct Pos(f32, f32);
//! struct Vel(f32, f32);
//!
//! let mut world = World::new();
//! let e = world.spawn();
//! world.insert(e, Pos(0.0, 0.0));
//! world.insert(e, Vel(1.0, 0.0));
//! for (pos, vel) in world.query::<(&Pos, &Vel)>() {
//!     // ...
//! }
//! world.despawn(e);
//! ```

pub mod actor;
pub mod archetype;
pub mod component;
pub mod component_store;
pub mod entity;
pub mod handle;
pub mod query;
pub mod registry;
pub mod schedule;
pub mod world;

pub use actor::{Actor, ActorRegistry};
pub use archetype::{Archetype, ArchetypeId, ArchetypeKey};
pub use component::{component_id, Component, ComponentId};
pub use component_store::{__bp_clear_comp_ctx, __bp_set_comp_ctx, __bp_with_comp, ComponentStore};
pub use entity::Entity;
pub use handle::Handle;
pub use pulsar_core::GameTime;
pub use query::{QueryIter, WorldQuery};
pub use registry::{HandleRegistry, NULL_ROW};
pub use schedule::Schedule;
pub use world::World;
