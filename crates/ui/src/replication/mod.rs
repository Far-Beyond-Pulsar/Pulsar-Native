//! Multi-user editing and state replication system
//!
//! This module provides the infrastructure for replicating UI component state
//! across multiple connected users in real-time collaborative editing sessions.
//!
//! ## Core Concepts
//!
//! - **ReplicationMode**: Defines how a UI element's state should be shared
//! - **Replicator**: Trait that makes components network-aware
//! - **ReplicationState**: Tracks which users are editing which elements
//! - **PresenceIndicator**: Visual feedback for multi-user interaction
//!
//! ## Example
//!
//! ```ignore
//! // Create a replicated text input
//! let input_state = cx.new(|cx| {
//!     InputState::new(window, cx)
//!         .with_replication(ReplicationMode::MultiEdit)
//! });
//!
//! // The input will now sync changes to all connected users
//! TextInput::new(&input_state)
//!     .placeholder("Shared field...")
//!     .render(cx)
//! ```

mod context;
mod extensions;
mod integration;
mod mode;
mod presence;
mod state;
mod sync;
mod traits;

pub use context::*;
pub use extensions::*;
pub use integration::*;
pub use mode::*;
pub use presence::*;
pub use state::*;
pub use sync::*;
pub use traits::*;

use gpui::App;

/// Initialize the replication system
pub fn init(cx: &mut App) {
    ReplicationRegistry::init(cx);
    SessionContext::init(cx);
}
