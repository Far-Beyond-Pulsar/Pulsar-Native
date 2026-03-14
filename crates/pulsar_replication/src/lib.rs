//! Multi-user editing and state replication engine
//!
//! Provides the infrastructure for replicating UI component state across
//! multiple connected users in real-time collaborative editing sessions.
//!
//! ## Core Concepts
//!
//! - [`ReplicationMode`]: Defines how an element's state should be shared
//! - [`Replicator`]: Trait that makes components network-aware
//! - [`ReplicationRegistry`]: Tracks which users are editing which elements
//! - [`UserPresence`]: Represents a connected user's state

mod context;
mod integration;
mod mode;
mod presence;
mod state;
mod sync;
mod traits;

pub use context::*;
pub use integration::*;
pub use mode::*;
pub use presence::*;
pub use state::*;
pub use sync::*;
pub use traits::*;

use gpui::App;

/// Initialize the replication system globals
pub fn init(cx: &mut App) {
    ReplicationRegistry::init(cx);
    SessionContext::init(cx);
}
