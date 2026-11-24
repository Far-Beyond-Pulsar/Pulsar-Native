//! The script-facing object model (scripting epic workstream B).
//!
//! Gameplay code -- native Rust actors, generated Blueprint code, VM
//! bytecode -- addresses the world exclusively through the lightweight
//! handles in [`refs`]: an [`ActorRef`] names one live entity, a
//! [`ComponentRef`] names one component instance on it via the properties
//! panel's identity convention, `(class_name, component_index)`.
//! Every accessor validates the handle against the
//! shared `pulsar_scenedb::World` *at each use* and reports invalid handles
//! as typed errors ([`errors::ScriptRefError`]) -- never panics, never
//! silently readdresses a recycled slot.
//!
//! ## Invariants every downstream consumer (C/D/E/F) can rely on
//!
//! - **One world.** Handles are meaningless without a world argument; all
//!   accessors take `&pulsar_scenedb::World`/`&mut World` explicitly (the
//!   same `Arc<RwLock<SceneDb>>` handle pattern handoff A
//!   established; callers pass `store.read().world()` / `.world_mut()`).
//! - **Validated per access.** A ref that was valid when stored may be stale
//!   when used; storing refs freely is safe and supported. Staleness is an
//!   ordinary, expected result (`ReferenceDespawned`), not misuse.
//! - **Never panics on bad handles.** Every accessor returns `Err` for dead,
//!   missing, or mismatched targets ([`errors`]), including the
//!   `Entity::DANGLING` sentinel (scripts' `entity::none()`); see
//!   [`contract`] for the full handle-semantics page.
//! - **One instance, one value.** Every attached component instance is its
//!   own entity holding its own typed value; a
//!   [`ComponentRef`] resolves to exactly one of them, and every accessor
//!   reads and writes that typed value.
//!
//! ## Module map
//!
//! | Module | Concern |
//! |---|---|
//! | [`refs`] | `ActorRef`/`ComponentRef` value types + liveness validation |
//! | [`errors`] | the typed error taxonomy |
//! | [`access`] | property/method accessors |
//! | [`subscribe`] | change watching over SceneDB change journals |
//! | [`resolution`] | StableId <-> Entity serialization + resolution |
//! | [`reflect`] | identity types through reflection registries |
//! | [`dispatch`] | demo dynamic-dispatch methods taking/returning refs |
//! | [`contract`] | handle semantics, one page, for script authors |

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
