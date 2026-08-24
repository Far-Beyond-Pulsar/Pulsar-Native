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
//!   missing, or mismatched targets.
//! - **Panel-parity routing.** Property reads/writes route exactly like the
//!   properties panel (#519/#575): the first enabled instance of a class is
//!   the *live-typed* value in `World`; every other index lives as JSON in
//!   its own instance record ([`instances::ComponentInstanceStore`]).
//!
//! ## Module map
//!
//! | Module | Concern |
//! |---|---|
//! | [`refs`] | `ActorRef`/`ComponentRef` value types + liveness validation |
//! | [`errors`] | the typed error taxonomy (#641) |
//! | [`instances`] | duplicate-instance storage seam (JSON records) |
//! | [`routing`] | live-typed-vs-duplicate routing internals |
//! | [`access`] | property/method accessors |
//! | [`subscribe`] | change-notification helpers over SceneDB#47 subscriptions |

pub mod access;
pub mod errors;
pub mod instances;
pub mod refs;
pub mod routing;
pub mod subscribe;

#[cfg(test)]
pub(crate) mod test_support;

pub use errors::ScriptRefError;
pub use instances::{ComponentInstanceStore, InstanceRecord};
pub use refs::{ActorRef, ComponentRef};
