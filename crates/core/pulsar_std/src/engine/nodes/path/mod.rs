//! Path Nodes
//!
//! Nodes for filesystem path operations in Pulsar blueprints.
//!
//! # Node Category: Path
//!
//! Provides utilities for joining, splitting, and normalizing paths.

use crate::blueprint;

/// Join two path segments.
///
/// # Inputs
/// - `a`: First path segment
/// - `b`: Second path segment
///
/// # Returns
/// The joined path.
///
/// # Path Join
/// Joins two path segments into a single path.
#[blueprint(type: crate::NodeTypes::pure, category: "Path")]
pub fn join_path(a: String, b: String) -> String {
    if a.ends_with(std::path::MAIN_SEPARATOR) {
        format!("{}{}", a, b)
    } else {
        format!("{}{}{}", a, std::path::MAIN_SEPARATOR, b)
    }
}

/// Split a path into its components (placeholder: returns the input string).
///
/// # Inputs
/// - `path`: The path to split
///
/// # Returns
/// The components (placeholder: string).
///
/// # Path Split
/// Splits a path into its components.
#[blueprint(type: crate::NodeTypes::pure, category: "Path")]
pub fn split_path(path: String) -> String {
    // In a real implementation, return Vec<String>
    path
}

/// Normalize a path string (placeholder: returns the input string).
///
/// # Inputs
/// - `path`: The path to normalize
///
/// # Returns
/// The normalized path (placeholder: unchanged).
///
/// # Path Normalize
/// Normalizes a path string.
#[blueprint(type: crate::NodeTypes::pure, category: "Path")]
pub fn normalize_path(path: String) -> String {
    path
}
