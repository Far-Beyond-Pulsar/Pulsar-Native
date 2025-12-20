//! Input handling module
//!
//! This module contains handlers for all input-related events including
//! keyboard, mouse, and modifier state changes.

pub mod keyboard;
pub mod mouse;
pub mod modifiers;

pub use keyboard::{handle_keyboard_input, keycode_to_string};
pub use mouse::{handle_cursor_moved, handle_mouse_input, handle_mouse_wheel};
pub use modifiers::handle_modifiers_changed;
