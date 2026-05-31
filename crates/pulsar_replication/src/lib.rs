//! Multi-user editing and state replication engine.
//!
//! All types are now canonical in `ui::replication`.  This crate re-exports
//! them so that existing engine code continues to work without changes.

pub use ui::replication::*;

use gpui::App;

/// Initialize the replication system globals.
pub fn init(cx: &mut App) {
    ui::replication::init(cx);
}
