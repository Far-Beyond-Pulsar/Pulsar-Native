//! Core UI Components and Utilities
//!
//! This module contains the fundamental building blocks of the UI system:
//! - Main application state and logic
//! - Shared utilities used across multiple UI components
//! - File system utilities for UI operations

pub mod app;
pub mod file_utils;
pub mod shared;

pub use app::{PulsarApp, PulsarRoot, ToggleCommandPalette};
pub use file_utils::*;
pub use shared::*;
