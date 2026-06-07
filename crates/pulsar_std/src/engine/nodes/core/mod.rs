//! # Core Module
//!
//! Literal/constant-value nodes for the Pulsar visual programming system.
//!
//! These are pure passthrough nodes that expose a single typed input as their
//! output — the visual-editor equivalent of a literal constant (e.g. Unreal's
//! "Make Literal Int"). They exist so graphs can supply typed constant values
//! to other nodes' input pins.

use crate::blueprint;

/// Make a literal 32-bit signed integer constant.
///
/// # Inputs
/// - `value`: The constant value
///
/// # Returns
/// The same value, unchanged
///
/// # Make Literal Int32
/// Outputs a constant 32-bit signed integer value.
#[blueprint(type: NodeTypes::pure, category: "Core", color: "#7F8C8D")]
pub fn make_literal_i32(value: i32) -> i32 {
    value
}

/// Make a literal 64-bit signed integer constant.
///
/// # Inputs
/// - `value`: The constant value
///
/// # Returns
/// The same value, unchanged
///
/// # Make Literal Int64
/// Outputs a constant 64-bit signed integer value.
#[blueprint(type: NodeTypes::pure, category: "Core", color: "#7F8C8D")]
pub fn make_literal_i64(value: i64) -> i64 {
    value
}

/// Make a literal 32-bit unsigned integer constant.
///
/// # Inputs
/// - `value`: The constant value
///
/// # Returns
/// The same value, unchanged
///
/// # Make Literal UInt32
/// Outputs a constant 32-bit unsigned integer value.
#[blueprint(type: NodeTypes::pure, category: "Core", color: "#7F8C8D")]
pub fn make_literal_u32(value: u32) -> u32 {
    value
}

/// Make a literal 64-bit unsigned integer constant.
///
/// # Inputs
/// - `value`: The constant value
///
/// # Returns
/// The same value, unchanged
///
/// # Make Literal UInt64
/// Outputs a constant 64-bit unsigned integer value.
#[blueprint(type: NodeTypes::pure, category: "Core", color: "#7F8C8D")]
pub fn make_literal_u64(value: u64) -> u64 {
    value
}

/// Make a literal 32-bit floating point constant.
///
/// # Inputs
/// - `value`: The constant value
///
/// # Returns
/// The same value, unchanged
///
/// # Make Literal Float
/// Outputs a constant 32-bit floating point value.
#[blueprint(type: NodeTypes::pure, category: "Core", color: "#7F8C8D")]
pub fn make_literal_f32(value: f32) -> f32 {
    value
}

/// Make a literal 64-bit floating point constant.
///
/// # Inputs
/// - `value`: The constant value
///
/// # Returns
/// The same value, unchanged
///
/// # Make Literal Double
/// Outputs a constant 64-bit floating point value.
#[blueprint(type: NodeTypes::pure, category: "Core", color: "#7F8C8D")]
pub fn make_literal_f64(value: f64) -> f64 {
    value
}

/// Make a literal boolean constant.
///
/// # Inputs
/// - `value`: The constant value
///
/// # Returns
/// The same value, unchanged
///
/// # Make Literal Bool
/// Outputs a constant boolean value.
#[blueprint(type: NodeTypes::pure, category: "Core", color: "#7F8C8D")]
pub fn make_literal_bool(value: bool) -> bool {
    value
}

/// Make a literal string constant.
///
/// # Inputs
/// - `value`: The constant value
///
/// # Returns
/// The same value, unchanged
///
/// # Make Literal String
/// Outputs a constant string value.
#[blueprint(type: NodeTypes::pure, category: "Core", color: "#7F8C8D")]
pub fn make_literal_string(value: String) -> String {
    value
}
