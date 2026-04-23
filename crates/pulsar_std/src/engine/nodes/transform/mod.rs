//! # Transform Module
//!
//! Transform operations and utilities for the Pulsar visual programming system.
//!
//! This module provides pure transform nodes including:
//! - Transform construction (transform_new, make_transform)
//! - Transform deconstruction (break_transform)
//!
//! ## Transform Representation
//!
//! Transforms are represented as nested tuples containing position, rotation, and scale:
//! `((f32, f32, f32), (f32, f32, f32), (f32, f32, f32))` for (position, rotation, scale)
//! Each component is a 3D vector (x, y, z).

use crate::blueprint;

// =============================================================================
// Transform Construction
// =============================================================================

/// Create a new transform with position, rotation, and scale.
///
/// This node constructs a transform tuple from three input vectors: position, rotation, and scale.
/// Each vector is expected to be a 3-tuple of numbers (f32).
///
/// # Inputs
/// - `position`: The position vector (x, y, z)
/// - `rotation`: The rotation vector (x, y, z)
/// - `scale`: The scale vector (x, y, z)
///
/// # Returns
/// A tuple containing position, rotation, and scale vectors: `((f32, f32, f32), (f32, f32, f32), (f32, f32, f32))`
///
/// # Example
/// If `position` is (1.0, 2.0, 3.0), `rotation` is (0.0, 0.0, 0.0), and `scale` is (1.0, 1.0, 1.0),
/// the output will be ((1.0, 2.0, 3.0), (0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).
///
/// # Notes
/// This node is useful for constructing transform objects for use in 3D graphics or spatial calculations.
/// # Transform New
/// Creates a new transform from position, rotation, and scale.
#[blueprint(type: NodeTypes::pure, category: "Transform", color: "#50E3C2")]
pub fn transform_new(
    position: (f32, f32, f32),
    rotation: (f32, f32, f32),
    scale: (f32, f32, f32),
) -> ((f32, f32, f32), (f32, f32, f32), (f32, f32, f32)) {
    (position, rotation, scale)
}

/// Create a transform from position, rotation, and scale components.
///
/// This node constructs a transform tuple from the provided position, rotation, and scale vectors.
/// Useful for 3D graphics, spatial calculations, and object manipulation.
///
/// # Inputs
/// - `position`: The position vector (tuple of three floats: x, y, z)
/// - `rotation`: The rotation vector (tuple of three floats: x, y, z)
/// - `scale`: The scale vector (tuple of three floats: x, y, z)
///
/// # Returns
/// A tuple representing the transform: (position, rotation, scale)
///
/// # Example
/// If `position` is (1.0, 2.0, 3.0), `rotation` is (0.0, 0.0, 0.0), and `scale` is (1.0, 1.0, 1.0),
/// the output will be ((1.0, 2.0, 3.0), (0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).
///
/// # Notes
/// Use this node to assemble transform data for 3D objects, physics, or rendering.
/// # Make Transform
/// Creates a transform from position, rotation, and scale components.
#[blueprint(type: NodeTypes::pure, category: "Transform", color: "#50E3C2")]
pub fn make_transform(
    position: (f32, f32, f32),
    rotation: (f32, f32, f32),
    scale: (f32, f32, f32),
) -> ((f32, f32, f32), (f32, f32, f32), (f32, f32, f32)) {
    (position, rotation, scale)
}

// =============================================================================
// Transform Deconstruction
// =============================================================================

/// Break a transform into position, rotation, and scale components.
///
/// This node takes a transform input and returns its position, rotation, and scale as separate components.
///
/// # Inputs
/// - `transform`: The input transform, typically a tuple of (position, rotation, scale)
///
/// # Returns
/// A tuple containing:
///   - Position (tuple of three numbers)
///   - Rotation (tuple of three numbers)
///   - Scale (tuple of three numbers)
///
/// # Example
/// If the input transform is ((1.0, 2.0, 3.0), (0.0, 0.0, 0.0), (1.0, 1.0, 1.0)),
/// the output will be (position: (1.0, 2.0, 3.0), rotation: (0.0, 0.0, 0.0), scale: (1.0, 1.0, 1.0)).
///
/// # Notes
/// Use this node to extract individual transform components for further manipulation.
/// # Break Transform
/// Breaks a transform into position, rotation, and scale components.
#[blueprint(type: NodeTypes::pure, category: "Transform", color: "#50E3C2")]
pub fn break_transform(
    transform: ((f32, f32, f32), (f32, f32, f32), (f32, f32, f32)),
) -> ((f32, f32, f32), (f32, f32, f32), (f32, f32, f32)) {
    let (position, rotation, scale) = transform;
    (position, rotation, scale)
}
