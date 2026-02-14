//! Input handling module
//!
//! This module contains handlers for all input-related events including
//! keyboard, mouse, and modifier state changes.

pub mod conversion;
pub mod keyboard;
pub mod mouse;
pub mod modifiers;

pub use conversion::{
    ToGpuiMouseButton, ToGpuiModifiers, keycode_to_string,
    is_in_titlebar_drag_area, get_resize_direction,
};
pub use keyboard::handle_keyboard_input;
pub use mouse::{handle_cursor_moved, handle_mouse_input, handle_mouse_wheel};
pub use modifiers::handle_modifiers_changed;
