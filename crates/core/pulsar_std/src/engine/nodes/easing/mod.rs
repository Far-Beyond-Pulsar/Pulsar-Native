//! Easing Nodes
//!
//! Nodes for animation easing and interpolation in Pulsar blueprints.
//!
//! # Node Category: Easing
//!
//! Provides utilities for smooth transitions and interpolation.

use crate::blueprint;

/// Linear interpolation between two values.
///
/// # Inputs
/// - `a`: Start value
/// - `b`: End value
/// - `t`: Interpolation factor (0.0 to 1.0)
///
/// # Returns
/// The interpolated value.
///
/// # Easing Lerp
/// Linearly interpolates between two values.
#[blueprint(type: crate::NodeTypes::pure, category: "Easing")]
pub fn ease_lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Ease-in interpolation.
///
/// # Inputs
/// - `t`: Interpolation factor (0.0 to 1.0)
///
/// # Returns
/// The eased value.
///
/// # Easing Ease In
/// Applies an ease-in curve.
#[blueprint(type: crate::NodeTypes::pure, category: "Easing")]
pub fn ease_in(t: f64) -> f64 {
    t * t
}

/// Ease-out interpolation.
///
/// # Inputs
/// - `t`: Interpolation factor (0.0 to 1.0)
///
/// # Returns
/// The eased value.
///
/// # Easing Ease Out
/// Applies an ease-out curve.
#[blueprint(type: crate::NodeTypes::pure, category: "Easing")]
pub fn ease_out(t: f64) -> f64 {
    t * (2.0 - t)
}
