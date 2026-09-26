//! Engine-wide event plumbing for Pulsar, on [Gamma v2](gamma).
//!
//! Three pieces:
//!
//! - [`hub`]: the **engine event hub**. One [`EventHub`] per world / game
//!   session (`pulsar_game::tick::TickLoop` owns it) wraps a Gamma
//!   `SyncEventBus`. Built-in events ([`builtin`]), script events and
//!   plugin events are published *deferred* and delivered when the tick
//!   loop flushes the hub at its fixed [`FlushPoint`]s. Script-declared
//!   events are registered on it as dynamic descriptors. A bounded debug
//!   [`tap`] records recently flushed events for the editor's PIE events
//!   panel.
//! - [`host`]: the **process-wide host bus**. In the editor (or game)
//!   binary it is a local `SyncEventBus`; a plugin loaded as a separate
//!   dynamic library receives the host's bus as a Gamma
//!   [`RawBus`](gamma::ffi::RawBus) when it is loaded
//!   ([`host::attach_host_bus`]) and uses it through `ForeignBus`, so there
//!   is one bus per process instead of one per copy of this crate
//!   (Pulsar-Native#930).
//! - [`assets`]: asset-update notifications ([`AssetUpdated`]) on the host
//!   bus. Anything that rewrites an asset publishes one; the level editor,
//!   the game runtime and plugins subscribe by [`AssetKind`].
//!
//! This crate is GPUI-free so the game runtime can use it; editor plugins
//! reach it through `plugin_editor_api`, which re-exports the asset API.

pub use gamma;

/// Profiler scope when the `profiling` feature is on; nothing otherwise.
macro_rules! scope {
    ($name:literal) => {
        #[cfg(feature = "profiling")]
        profiling::profile_scope!($name);
    };
}


pub mod assets;
pub mod builtin;
pub mod channel;
pub mod foreign_tap;
pub mod host;
pub mod hub;
pub mod problems;
pub mod session;
pub mod tap;

pub use assets::{AssetSubscription, AssetUpdated, publish_asset_updated, subscribe_asset_updates};
pub use problems::{
    ProblemSeverity, ScriptProblem, ScriptProblemsEvent, publish_script_problem, publish_script_problems_cleared,
    subscribe_script_problems,
};
pub use session::{PieSessionEvent, announce_session_started, announce_session_stopping, subscribe_pie_sessions};
pub use channel::{class_channel, class_channel_id, entity_channel};
pub use hub::{EventCategory, EventHub, EventInfo, FlushPoint};
pub use tap::{EventsSnapshot, SnapshotEvent, SnapshotRecord, TapRecord};
pub use ui_types_common::AssetKind;
