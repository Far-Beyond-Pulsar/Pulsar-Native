//! Platform-specific cursor and input handling.
//!
//! This module provides cross-platform cursor locking, hiding, and positioning
//! for viewport camera controls. Each platform (Windows, macOS, Linux) has its
//! own implementation using native APIs for precise cursor control.

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod x11;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use x11::*;

#[cfg(not(target_os = "macos"))]
pub fn prepare_relative_mouse_mode() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn begin_relative_mouse_mode() {}

#[cfg(not(target_os = "macos"))]
pub fn end_relative_mouse_mode() {}

#[cfg(not(target_os = "macos"))]
pub fn take_mouse_delta() -> (f32, f32) {
    (0.0, 0.0)
}
