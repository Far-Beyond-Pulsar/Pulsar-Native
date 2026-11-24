//! Per-game lifecycle reconciliation for live components.
//!
//! One registration feeds it: the capability registry's per-class
//! `ActorTable` ([`pulsar_component_lifecycle`]), which this module drives
//! instance by instance in `Step::Components`. A table comes from a class's
//! `ClassSpec`, from an engine crate (`EngineActor`) or from
//! `#[register_component_runtime]`, whose expansion is a table with native
//! handler events.
//!
//! This state stores identities, event subscriptions, and transient payload
//! inboxes only. The authoritative component values remain in SceneDB and
//! are borrowed by each generated typed query for callbacks.

mod construct;
mod session;
mod summaries;
#[cfg(test)]
mod tests;
mod tick;

pub use construct::pump_construct;
pub use session::{ComponentInstanceKey, ComponentRuntimeState, ConstructStats, Session};
pub use tick::{
    end_live_components, end_session, process_component_removals, process_component_removals_mut,
    tick_live_components,
};
