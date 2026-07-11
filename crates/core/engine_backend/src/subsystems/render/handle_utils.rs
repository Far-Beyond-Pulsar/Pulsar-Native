//! DXGI Handle Utilities
//!
//! Provides platform-specific handle conversion utilities for DXGI shared textures.
//! Consolidates handle conversion logic used across the render subsystem.

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HANDLE;

/// Convert a Windows HANDLE to usize for storage
///
/// This allows HANDLE values to be stored in platform-independent data structures.
///
/// # Arguments
/// * `handle` - The Windows HANDLE to convert
///
/// # Returns
/// The handle value as a usize
#[cfg(target_os = "windows")]
pub fn handle_to_usize(handle: HANDLE) -> usize {
    handle.0 as usize
}

/// Convert a usize back to Windows HANDLE
///
/// Reconstructs a HANDLE from a stored usize value.
///
/// # Arguments
/// * `value` - The usize value to convert
///
/// # Returns
/// A Windows HANDLE
#[cfg(target_os = "windows")]
pub fn usize_to_handle(value: usize) -> HANDLE {
    HANDLE(value as *mut core::ffi::c_void)
}

/// Non-Windows stub: Pass through usize values
///
/// On non-Windows platforms, handles are already represented as usize.
#[cfg(not(target_os = "windows"))]
pub fn handle_to_usize(handle: usize) -> usize {
    handle
}

/// Non-Windows stub: Pass through usize values
///
/// On non-Windows platforms, handles are already represented as usize.
#[cfg(not(target_os = "windows"))]
pub fn usize_to_handle(value: usize) -> usize {
    value
}
