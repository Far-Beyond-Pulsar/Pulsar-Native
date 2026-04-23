//! # Color Module
//!
//! Color operations and utilities for the Pulsar visual programming system.
//!
//! This module provides pure color nodes including:
//! - Color construction (color_new, make_color)
//! - Color deconstruction (break_color)
//! - Color operations (color_lerp)
//!
//! ## Color Representation
//!
//! Colors are represented as RGBA tuples: `(f32, f32, f32, f32)` for (red, green, blue, alpha)
//! Each component typically ranges from 0.0 to 1.0.

use crate::blueprint;

// =============================================================================
// Color Construction
// =============================================================================

/// Create a new color from RGBA components.
///
/// This node constructs a color value using the provided red, green, blue, and alpha components.
///
/// # Inputs
/// - `r`: The red component (f32, typically 0.0 to 1.0)
/// - `g`: The green component (f32, typically 0.0 to 1.0)
/// - `b`: The blue component (f32, typically 0.0 to 1.0)
/// - `a`: The alpha (opacity) component (f32, typically 0.0 to 1.0)
///
/// # Returns
/// A tuple representing the color: (r, g, b, a)
///
/// # Example
/// If r = 1.0, g = 0.5, b = 0.0, a = 1.0, the output will be (1.0, 0.5, 0.0, 1.0).
///
/// # Notes
/// Use this node to construct colors for rendering, UI, or data visualization.
/// # Color New
/// Creates a new color from RGBA components.
#[blueprint(type: NodeTypes::pure, category: "Color", color: "#FF6B9D")]
pub fn color_new(r: f32, g: f32, b: f32, a: f32) -> (f32, f32, f32, f32) {
    (r, g, b, a)
}

/// Create a color from R, G, B, A components.
///
/// This node constructs a color value using the provided red, green, blue, and alpha components.
/// Useful for graphics, UI, rendering, or any context where color values are needed.
///
/// # Inputs
/// - `r`: The red component (f32, typically 0.0 to 1.0)
/// - `g`: The green component (f32, typically 0.0 to 1.0)
/// - `b`: The blue component (f32, typically 0.0 to 1.0)
/// - `a`: The alpha (opacity) component (f32, typically 0.0 to 1.0)
///
/// # Returns
/// A tuple representing the color: (r, g, b, a)
///
/// # Example
/// If r = 1.0, g = 0.5, b = 0.0, a = 1.0, the output will be (1.0, 0.5, 0.0, 1.0).
///
/// # Notes
/// Use this node to construct colors for rendering, UI, or data visualization.
/// # Make Color
/// Creates a color from R, G, B, A components.
#[blueprint(type: NodeTypes::pure, category: "Color", color: "#FF6B9D")]
pub fn make_color(r: f32, g: f32, b: f32, a: f32) -> (f32, f32, f32, f32) {
    (r, g, b, a)
}

// =============================================================================
// Color Deconstruction
// =============================================================================

/// Break a color into its RGBA components.
///
/// This node takes a color input and returns its individual red, green, blue, and alpha components as a tuple.
///
/// # Inputs
/// - `color`: The input color (f32, f32, f32, f32)
///
/// # Returns
/// A tuple of four numbers: (red, green, blue, alpha)
///
/// # Example
/// If `color` is (0.5, 0.2, 0.8, 1.0), the output will be (0.5, 0.2, 0.8, 1.0).
///
/// # Notes
/// Useful for extracting color channels for further processing or analysis.
/// # Break Color
/// Breaks a color into R, G, B, A components.
#[blueprint(type: NodeTypes::pure, category: "Color", color: "#FF6B9D")]
pub fn break_color(color: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (r, g, b, a) = color;
    (r, g, b, a)
}

// =============================================================================
// Color Operations
// =============================================================================

/// Linearly interpolate between two colors.
///
/// This node takes two color inputs (each as RGBA tuples) and a parameter t, and returns a new color
/// that is the linear interpolation between the two colors. The interpolation is performed component-wise.
///
/// # Inputs
/// - `a`: The first color (tuple of four floats: r, g, b, a)
/// - `b`: The second color (tuple of four floats: r, g, b, a)
/// - `t`: The interpolation parameter (f32, typically in [0, 1])
///
/// # Returns
/// A new color (tuple of four floats) representing the interpolated RGBA value
///
/// # Example
/// If `a` is (1.0, 0.0, 0.0, 1.0) and `b` is (0.0, 0.0, 1.0, 1.0) and `t` is 0.5,
/// the output will be (0.5, 0.0, 0.5, 1.0).
///
/// # Notes
/// The parameter t is not clamped; values outside [0, 1] will extrapolate.
/// Useful for color blending, gradients, and transitions.
/// # Color Lerp
/// Linearly interpolates between two colors using parameter t.
#[blueprint(type: NodeTypes::pure, category: "Color", color: "#FF6B9D")]
pub fn color_lerp(
    a: (f32, f32, f32, f32),
    b: (f32, f32, f32, f32),
    t: f32,
) -> (f32, f32, f32, f32) {
    let (r1, g1, b1, a1) = a;
    let (r2, g2, b2, a2) = b;
    (
        r1 + (r2 - r1) * t,
        g1 + (g2 - g1) * t,
        b1 + (b2 - b1) * t,
        a1 + (a2 - a1) * t,
    )
}
