//! Validation Nodes
//!
//! Nodes for data validation in Pulsar blueprints.
//!
//! # Node Category: Validation
//!
//! Provides utilities for checking strings, numbers, and ranges.

use crate::blueprint;

/// Check if a string is a valid email (very basic check).
///
/// # Inputs
/// - `s`: The string to check
///
/// # Returns
/// True if the string contains '@' and '.', false otherwise.
///
/// # Validation Is Email
/// Checks if a string is a valid email address.
#[blueprint(type: crate::NodeTypes::pure, category: "Validation")]
pub fn is_email(s: String) -> bool {
    s.contains('@') && s.contains('.')
}

/// Check if a string is a valid number.
///
/// # Inputs
/// - `s`: The string to check
///
/// # Returns
/// True if the string parses as a number.
///
/// # Validation Is Number
/// Checks if a string is a valid number.
#[blueprint(type: crate::NodeTypes::pure, category: "Validation")]
pub fn is_number(s: String) -> bool {
    s.parse::<f64>().is_ok()
}

/// Check if a value is within a range.
///
/// # Inputs
/// - `value`: The value to check
/// - `min`: Minimum allowed value
/// - `max`: Maximum allowed value
///
/// # Returns
/// True if value is in [min, max].
///
/// # Validation Range
/// Checks if a value is within a range.
#[blueprint(type: crate::NodeTypes::pure, category: "Validation")]
pub fn validate_range(value: f64, min: f64, max: f64) -> bool {
    value >= min && value <= max
}
