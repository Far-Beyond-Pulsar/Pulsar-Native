//! Input Conversion Utilities
//!
//! Provides utilities for converting Winit input types to GPUI types,
//! and helper functions for window manipulation detection (titlebar dragging, resize areas).
//!
//! ## Conversion Functions
//! - Mouse button conversion (Winit → GPUI)
//! - Keyboard modifier conversion (Winit → GPUI)
//! - KeyCode to string mapping
//!
//! ## Window Manipulation Helpers
//! - Titlebar drag area detection
//! - Window resize direction detection

use gpui::*;
use winit::event::MouseButton as WinitMouseButton;
use winit::keyboard::KeyCode;
use winit::window::ResizeDirection;

// ============================================================================
// Extension Traits for Type Conversion
// ============================================================================

/// Extension trait for converting Winit MouseButton to GPUI MouseButton
///
/// Provides idiomatic `.to_gpui()` method for type conversion.
pub trait ToGpuiMouseButton {
    fn to_gpui(self) -> MouseButton;
}

impl ToGpuiMouseButton for WinitMouseButton {
    fn to_gpui(self) -> MouseButton {
        match self {
            WinitMouseButton::Left => MouseButton::Left,
            WinitMouseButton::Right => MouseButton::Right,
            WinitMouseButton::Middle => MouseButton::Middle,
            WinitMouseButton::Back => MouseButton::Navigate(NavigationDirection::Back),
            WinitMouseButton::Forward => MouseButton::Navigate(NavigationDirection::Forward),
            WinitMouseButton::Other(_) => MouseButton::Left, // Fallback
        }
    }
}

/// Extension trait for converting Winit modifiers to GPUI modifiers
///
/// Provides idiomatic `.to_gpui()` method for type conversion.
pub trait ToGpuiModifiers {
    fn to_gpui(&self) -> Modifiers;
}

impl ToGpuiModifiers for winit::keyboard::ModifiersState {
    fn to_gpui(&self) -> Modifiers {
        Modifiers {
            control: self.control_key(),
            alt: self.alt_key(),
            shift: self.shift_key(),
            platform: self.super_key(), // Windows key on Windows, Command on Mac
            function: false,            // Winit doesn't track function key separately
        }
    }
}

// ============================================================================
// KeyCode to String Conversion
// ============================================================================

/// Convert a Winit KeyCode to a string representation for GPUI
///
/// Maps physical keycodes to their string representations used by GPUI's
/// keystroke system. Returns None for unsupported keys.
///
/// # Arguments
/// * `code` - The KeyCode to convert
///
/// # Returns
/// Some(String) if the key is supported, None otherwise
pub fn keycode_to_string(code: KeyCode) -> Option<String> {
    use KeyCode::*;
    Some(match code {
        // Letters
        KeyA => "a",
        KeyB => "b",
        KeyC => "c",
        KeyD => "d",
        KeyE => "e",
        KeyF => "f",
        KeyG => "g",
        KeyH => "h",
        KeyI => "i",
        KeyJ => "j",
        KeyK => "k",
        KeyL => "l",
        KeyM => "m",
        KeyN => "n",
        KeyO => "o",
        KeyP => "p",
        KeyQ => "q",
        KeyR => "r",
        KeyS => "s",
        KeyT => "t",
        KeyU => "u",
        KeyV => "v",
        KeyW => "w",
        KeyX => "x",
        KeyY => "y",
        KeyZ => "z",

        // Numbers
        Digit0 => "0",
        Digit1 => "1",
        Digit2 => "2",
        Digit3 => "3",
        Digit4 => "4",
        Digit5 => "5",
        Digit6 => "6",
        Digit7 => "7",
        Digit8 => "8",
        Digit9 => "9",

        // Special keys
        Space => "space",
        Enter => "enter",
        Tab => "tab",
        Backspace => "backspace",
        Escape => "escape",
        Delete => "delete",
        Insert => "insert",
        Home => "home",
        End => "end",
        PageUp => "pageup",
        PageDown => "pagedown",

        // Arrow keys
        ArrowUp => "up",
        ArrowDown => "down",
        ArrowLeft => "left",
        ArrowRight => "right",

        // Function keys
        F1 => "f1",
        F2 => "f2",
        F3 => "f3",
        F4 => "f4",
        F5 => "f5",
        F6 => "f6",
        F7 => "f7",
        F8 => "f8",
        F9 => "f9",
        F10 => "f10",
        F11 => "f11",
        F12 => "f12",

        // Punctuation and symbols
        Minus => "-",
        Equal => "=",
        BracketLeft => "[",
        BracketRight => "]",
        Backslash => "\\",
        Semicolon => ";",
        Quote => "'",
        Comma => ",",
        Period => ".",
        Slash => "/",
        Backquote => "`",

        _ => return None,
    }.to_string())
}

// ============================================================================
// Window Manipulation Helpers
// ============================================================================

// Constants for window manipulation (in logical pixels, will be scaled)
const TITLEBAR_HEIGHT_LOGICAL: f64 = 34.0;  // Match TitleBar::TITLE_BAR_HEIGHT
const RESIZE_BORDER: f64 = 8.0;     // Size of resize grip area in physical pixels
const WINDOW_CONTROLS_WIDTH_LOGICAL: f64 = 138.0;  // Width of min/max/close buttons (3 * 46px)

/// Determine if a position is in the titlebar drag area
///
/// # Arguments
/// * `x` - X position in physical pixels
/// * `y` - Y position in physical pixels
/// * `window_width` - Window width in physical pixels
/// * `scale_factor` - Display scale factor
/// * `is_maximized` - Whether the window is maximized
///
/// # Returns
/// `true` if the position is in the draggable titlebar area
pub fn is_in_titlebar_drag_area(
    x: f64,
    y: f64,
    window_width: f64,
    scale_factor: f64,
    is_maximized: bool,
) -> bool {
    if is_maximized {
        return false;  // Can't drag maximized windows
    }

    let titlebar_height_physical = TITLEBAR_HEIGHT_LOGICAL * scale_factor;
    let controls_width_physical = WINDOW_CONTROLS_WIDTH_LOGICAL * scale_factor;

    // In titlebar height
    if y > titlebar_height_physical {
        return false;
    }

    // Not in window controls area (right side)
    if x > window_width - controls_width_physical {
        return false;
    }

    true
}

/// Determine resize direction based on cursor position
///
/// # Arguments
/// * `x` - X position in physical pixels
/// * `y` - Y position in physical pixels
/// * `window_width` - Window width in physical pixels
/// * `window_height` - Window height in physical pixels
/// * `is_maximized` - Whether the window is maximized
///
/// # Returns
/// Resize direction if cursor is in a resize area, None otherwise
pub fn get_resize_direction(
    x: f64,
    y: f64,
    window_width: f64,
    window_height: f64,
    is_maximized: bool,
) -> Option<ResizeDirection> {
    if is_maximized {
        return None;  // Can't resize maximized windows
    }

    let on_left = x < RESIZE_BORDER;
    let on_right = x > window_width - RESIZE_BORDER;
    let on_top = y < RESIZE_BORDER;
    let on_bottom = y > window_height - RESIZE_BORDER;

    // Corners take priority
    if on_top && on_left {
        return Some(ResizeDirection::NorthWest);
    }
    if on_top && on_right {
        return Some(ResizeDirection::NorthEast);
    }
    if on_bottom && on_left {
        return Some(ResizeDirection::SouthWest);
    }
    if on_bottom && on_right {
        return Some(ResizeDirection::SouthEast);
    }

    // Edges
    if on_top {
        return Some(ResizeDirection::North);
    }
    if on_bottom {
        return Some(ResizeDirection::South);
    }
    if on_left {
        return Some(ResizeDirection::West);
    }
    if on_right {
        return Some(ResizeDirection::East);
    }

    None
}
