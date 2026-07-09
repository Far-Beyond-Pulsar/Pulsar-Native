//! # Rect Module
//!
//! Rectangle operations and utilities for the Pulsar visual programming system.
//!
//! This module provides pure rectangle nodes including:
//! - Rectangle construction (rect_new, make_rect)
//! - Rectangle deconstruction (break_rect)
//! - Rectangle operations (rect_contains)
//!
//! ## Rectangle Representation
//!
//! Rectangles are represented as nested tuples containing position and size:
//! `((f32, f32), (f32, f32))` for (position, size)
//! - Position is a 2D vector (x, y)
//! - Size is a 2D vector (width, height)

use crate::blueprint;

// =============================================================================
// Rectangle Construction
// =============================================================================

/// Create a new rectangle from position and size components.
///
/// This node constructs a rectangle using the provided position and size vectors.
/// Useful for geometry, UI layout, collision detection, and spatial calculations.
///
/// # Inputs
/// - `position`: The position of the rectangle (tuple of two floats: x, y)
/// - `size`: The size of the rectangle (tuple of two floats: width, height)
///
/// # Returns
/// A tuple representing the rectangle: (position, size)
///
/// # Example
/// If `position` is (10.0, 20.0) and `size` is (100.0, 50.0), the output will be ((10.0, 20.0), (100.0, 50.0)).
///
/// # Notes
/// Use this node to define rectangles for rendering, hitboxes, or layout regions.
/// # Rect New
/// Creates a new rectangle from position and size.
#[blueprint(type: NodeTypes::pure, category: "Rect", color: "#F5A623")]
pub fn rect_new(position: (f32, f32), size: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    (position, size)
}

/// Create a rectangle from position and size components.
///
/// This node constructs a rectangle using the provided position and size vectors.
/// Useful for geometry, UI layout, collision detection, and spatial calculations.
///
/// # Inputs
/// - `position`: The position of the rectangle (tuple of two floats: x, y)
/// - `size`: The size of the rectangle (tuple of two floats: width, height)
///
/// # Returns
/// A tuple representing the rectangle: (position, size)
///
/// # Example
/// If `position` is (10.0, 20.0) and `size` is (100.0, 50.0), the output will be ((10.0, 20.0), (100.0, 50.0)).
///
/// # Notes
/// Use this node to define rectangles for rendering, hitboxes, or layout regions.
/// # Make Rect
/// Creates a rectangle from position and size components.
#[blueprint(type: NodeTypes::pure, category: "Rect", color: "#F5A623")]
pub fn make_rect(position: (f32, f32), size: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    (position, size)
}

// =============================================================================
// Rectangle Deconstruction
// =============================================================================

/// Break a rectangle into its position and size components.
///
/// This node takes a rectangle input and returns its position and size as separate values.
/// Useful for extracting individual properties from a rectangle structure.
///
/// # Inputs
/// - `rect`: The rectangle to break (typically a tuple or struct containing position and size)
///
/// # Returns
/// A tuple containing the position (x, y) and size (width, height) as separate tuples
///
/// # Example
/// If `rect` is ((10.0, 20.0), (100.0, 200.0)), the output will be ((10.0, 20.0), (100.0, 200.0)).
///
/// # Notes
/// The format of the rectangle must match the expected input type for correct extraction.
/// # Break Rect
/// Breaks a rectangle into position and size components.
#[blueprint(type: NodeTypes::pure, category: "Rect", color: "#F5A623")]
pub fn break_rect(rect: ((f32, f32), (f32, f32))) -> ((f32, f32), (f32, f32)) {
    let (position, size) = rect;
    (position, size)
}

// =============================================================================
// Rectangle Operations
// =============================================================================

/// Check if a rectangle contains a point.
///
/// This node determines whether a given point (x, y) lies within the bounds of a rectangle defined by its position and size.
/// Useful for hit-testing, collision detection, UI logic, and spatial queries.
///
/// # Inputs
/// - `rect`: The rectangle, represented as ((x, y), (width, height))
/// - `point`: The point to check, represented as (x, y)
///
/// # Returns
/// Returns `true` if the point is inside or on the edge of the rectangle, `false` otherwise
///
/// # Example
/// If `rect` is ((0.0, 0.0), (10.0, 5.0)) and `point` is (3.0, 4.0), the output will be true.
/// If the point is (11.0, 2.0), the output will be false.
///
/// # Notes
/// The check is inclusive: points on the edge of the rectangle are considered inside.
/// Use this node for geometric queries, UI hit-testing, or spatial logic.
/// # Rect Contains
/// Checks if a rectangle contains a point.
#[blueprint(type: NodeTypes::pure, category: "Rect", color: "#F5A623")]
pub fn rect_contains(rect: ((f32, f32), (f32, f32)), point: (f32, f32)) -> bool {
    let ((rect_x, rect_y), (width, height)) = rect;
    let (point_x, point_y) = point;
    point_x >= rect_x
        && point_x <= rect_x + width
        && point_y >= rect_y
        && point_y <= rect_y + height
}
