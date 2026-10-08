//! The script-facing object model (scripting epic workstream B, issues
//! #640/#639/#641/#642 under the [#633](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/633)
//! epic).
//!
//! Gameplay code -- native Rust actors, generated Blueprint code, VM
//! bytecode -- addresses the world exclusively through the lightweight
//! handles in [`refs`]: an [`ActorRef`] names one live entity, a
//! [`ComponentRef`] names one component instance on it via the properties
//! panel's identity convention, `(class_name, component_index)`
//! (Pulsar-Native#519/#575). Every accessor validates the handle against the
//! shared `pulsar_scenedb::World` *at each use* and reports invalid handles
//! as typed errors ([`errors::ScriptRefError`]) -- never panics, never
//! silently readdresses a recycled slot.
//!
//! ## Invariants every downstream consumer (C/D/E/F) can rely on
//!
//! - **One world.** Handles are meaningless without a world argument; all
//!   accessors take `&pulsar_scenedb::World`/`&mut World` explicitly (the
//!   same `Arc<RwLock<WorldSceneStore>>` handle pattern handoff A
//!   established; callers pass `store.read().world()` / `.world_mut()`).
//! - **Validated per access.** A ref that was valid when stored may be stale
//!   when used; storing refs freely is safe and supported. Staleness is an
//!   ordinary, expected result (`ReferenceDespawned`), not misuse.
//! - **Never panics on bad handles.** Every accessor returns `Err` for dead,
//!   missing, or mismatched targets ([`errors`]), including the
//!   `Entity::DANGLING` sentinel (scripts' `entity::none()`, #888); see
//!   [`contract`] for the full handle-semantics page (#641).
//! - **One instance, one value.** Every attached component instance is its
//!   own entity holding its own typed value (Pulsar-Native#1035, D1); a
//!   [`ComponentRef`] resolves to exactly one of them, and every accessor
//!   reads and writes that typed value.
//!
//! ## Module map
//!
//! | Module | Concern |
//! |---|---|
//! | [`refs`] | `ActorRef`/`ComponentRef` value types + liveness validation |
//! | [`errors`] | the typed error taxonomy (#641) |
//! | [`access`] | property/method accessors |
//! | [`subscribe`] | change watching over SceneDB change journals |
//! | [`resolution`] | StableId <-> Entity serialization + resolution (#639) |
//! | [`reflect`] | identity types through reflection registries (#642) |
//! | [`dispatch`] | demo dynamic-dispatch methods taking/returning refs (#642) |
//! | [`contract`] | handle semantics, one page, for script authors (#641) |

pub use resolution::{ResolveRefError, SerializedComponentRef, StableIdResolver};

pub mod access;
pub mod contract;
pub mod dispatch;
pub mod errors;
pub mod reflect;
pub mod refs;
pub mod resolution;
pub mod subscribe;
pub mod world_host;

#[cfg(test)]
mod property_tests;

#[cfg(test)]
pub(crate) mod test_support;

pub use errors::ScriptRefError;
pub use refs::{ActorRef, ComponentRef};
