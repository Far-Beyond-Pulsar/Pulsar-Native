//! Multi-user status bar indicator with profile picture display

mod components;
mod screen;
mod utils;

pub use screen::{AvatarCache, fetch_avatar_image, render_status_bar_indicator};
