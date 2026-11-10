//! Multi-user status bar indicator with profile picture display
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

mod components;
mod screen;
mod utils;

pub use screen::{AvatarCache, fetch_avatar_image, render_status_bar_indicator};
