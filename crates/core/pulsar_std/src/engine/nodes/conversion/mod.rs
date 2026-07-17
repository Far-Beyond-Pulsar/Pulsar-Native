//! Conversion Nodes
//!
//! Nodes for type conversions in Pulsar blueprints.
//!
//! Each node carries `#[conversion]` metadata so that the compiler can
//! auto-insert conversion nodes when connecting mismatched pin types.
//!
//! # Node Category: Conversion

use crate::{blueprint, conversion};

/// Convert a string to an integer (simple version, returns 0 on error).
///
/// # Inputs
/// - `s`: The string to convert
///
/// # Returns
/// The integer value, or 0 if parsing fails.
///
/// # Conversion Parse Int
/// Converts a string to an integer (returns 0 on error).
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn parse_int(s: String) -> i64 {
    s.parse().unwrap_or(0)
}

/// Convert bytes to a string (UTF-8, placeholder: returns empty string).
///
/// # Inputs
/// - `bytes`: The bytes as a string (placeholder)
///
/// # Returns
/// The string value (placeholder: empty string).
///
/// # Conversion Bytes to String
/// Converts bytes to a string.
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn bytes_to_string(_bytes: String) -> String {
    String::new()
}

// ── Numeric conversions ─────────────────────────────────────────────────────

/// Convert i64 to f64 (lossless).
#[conversion(from = "i64", to = "f64", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn i64_to_f64(value: i64) -> f64 {
    value as f64
}

/// Convert f64 to f32 (lossy).
#[conversion(from = "f64", to = "f32", lossless = false)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn f64_to_f32(value: f64) -> f32 {
    value as f32
}

/// Convert i64 to i32 (lossy, truncates).
#[conversion(from = "i64", to = "i32", lossless = false)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn i64_to_i32(value: i64) -> i32 {
    value as i32
}

/// Convert i64 to u32 (lossy, truncates negative).
#[conversion(from = "i64", to = "u32", lossless = false)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn i64_to_u32(value: i64) -> u32 {
    value as u32
}

/// Convert f32 to f64 (lossless).
#[conversion(from = "f32", to = "f64", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn f32_to_f64(value: f32) -> f64 {
    value as f64
}

/// Convert i32 to i64 (lossless).
#[conversion(from = "i32", to = "i64", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn i32_to_i64(value: i32) -> i64 {
    value as i64
}

/// Convert i32 to f64 (lossless for 32-bit int range).
#[conversion(from = "i32", to = "f64", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn i32_to_f64(value: i32) -> f64 {
    value as f64
}

/// Convert i32 to f32 (lossy for large values).
#[conversion(from = "i32", to = "f32", lossless = false)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn i32_to_f32(value: i32) -> f32 {
    value as f32
}

/// Convert i64 to f32 (lossy for large values).
#[conversion(from = "i64", to = "f32", lossless = false)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn i64_to_f32(value: i64) -> f32 {
    value as f32
}

// ── Vector/color/rect/transform conversions ─────────────────────────────────

/// Convert vec2 to vec3 (lossless, z=0).
#[conversion(from = "(f32, f32)", to = "(f32, f32, f32)", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn vec2_to_vec3(v: (f32, f32)) -> (f32, f32, f32) {
    (v.0, v.1, 0.0)
}

/// Convert vec3 to vec2 (lossy, drops z).
#[conversion(from = "(f32, f32, f32)", to = "(f32, f32)", lossless = false)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn vec3_to_vec2(v: (f32, f32, f32)) -> (f32, f32) {
    (v.0, v.1)
}

/// Convert vec2 to vec4 (lossless, z=0, w=1).
#[conversion(from = "(f32, f32)", to = "(f32, f32, f32, f32)", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn vec2_to_vec4(v: (f32, f32)) -> (f32, f32, f32, f32) {
    (v.0, v.1, 0.0, 1.0)
}

/// Convert vec3 to vec4 (lossless, w=1).
#[conversion(from = "(f32, f32, f32)", to = "(f32, f32, f32, f32)", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn vec3_to_vec4(v: (f32, f32, f32)) -> (f32, f32, f32, f32) {
    (v.0, v.1, v.2, 1.0)
}

/// Convert f32 to vec2 (lossless, broadcasts).
#[conversion(from = "f32", to = "(f32, f32)", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn f32_to_vec2(value: f32) -> (f32, f32) {
    (value, value)
}

/// Convert f32 to vec3 (lossless, broadcasts).
#[conversion(from = "f32", to = "(f32, f32, f32)", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn f32_to_vec3(value: f32) -> (f32, f32, f32) {
    (value, value, value)
}

/// Convert f32 to vec4 (lossless, broadcasts, alpha=1).
#[conversion(from = "f32", to = "(f32, f32, f32, f32)", lossless = true)]
#[blueprint(type: crate::NodeTypes::pure, category: "Conversion")]
pub fn f32_to_vec4(value: f32) -> (f32, f32, f32, f32) {
    (value, value, value, 1.0)
}
