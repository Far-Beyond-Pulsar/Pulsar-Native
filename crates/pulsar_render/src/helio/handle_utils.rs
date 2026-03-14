//! HANDLE ↔ usize conversion utilities.
//!
//! Provides platform-specific conversions so that native OS handles can be
//! stored in platform-independent data structures.

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HANDLE;

/// Convert a Windows `HANDLE` to `usize` for storage.
#[cfg(target_os = "windows")]
pub fn handle_to_usize(handle: HANDLE) -> usize {
    handle.0 as usize
}

/// Reconstruct a Windows `HANDLE` from a stored `usize`.
#[cfg(target_os = "windows")]
pub fn usize_to_handle(value: usize) -> HANDLE {
    HANDLE(value as *mut core::ffi::c_void)
}

/// Non-Windows stub: handles are already plain `usize` values.
#[cfg(not(target_os = "windows"))]
pub fn handle_to_usize(handle: usize) -> usize {
    handle
}

/// Non-Windows stub: handles are already plain `usize` values.
#[cfg(not(target_os = "windows"))]
pub fn usize_to_handle(value: usize) -> usize {
    value
}
