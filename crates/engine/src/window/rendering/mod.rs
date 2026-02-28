//! Rendering and composition module
//!
//! This module contains all rendering-related handlers including the main
//! compositor, Bevy integration, and resize handling.

pub mod compositor;
pub mod bevy;
pub mod resize;

pub use compositor::handle_redraw;
pub use resize::handle_resize;
