#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![warn(clippy::undocumented_unsafe_blocks)]

pub mod event;
pub mod task;
pub mod tick;
pub mod time;

pub use event::{EventBuffer, EventReader, EventWriter};
pub use task::TaskPool;
pub use tick::TickMode;
pub use time::{Clock, GameTime};
