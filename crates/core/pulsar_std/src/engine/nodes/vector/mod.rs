//! # Vector Module
//!
//! Vector operations and utilities for the Pulsar visual programming system.
//!
//! This module provides pure vector nodes including:
//! - Vector construction (make_vector2, make_vector3, vector2_new, vector3_new)
//! - Vector deconstruction (break_vector2, break_vector3)
//! - Component access (get_vector_x, get_vector_y, get_vector_z)
//! - Component modification (set_vector_x, set_vector_y)
//! - Vector arithmetic (vector2_add, vector3_add)
//! - Vector operations (dot product, cross product, magnitude, normalize)
//!
//! ## Vector Representation
//!
//! - 2D vectors are represented as tuples: `(f32, f32)` for (x, y)
//! - 3D vectors are represented as tuples: `(f32, f32, f32)` for (x, y, z)

use crate::blueprint;

// =============================================================================
// Vector2 Construction
// =============================================================================

/// Creates a 2D vector from X and Y components.
///
/// This node constructs a 2D vector (tuple of two floats) from the provided X and Y components.
/// Useful for geometry, graphics, physics, and any context where 2D positions or directions are needed.
///
/// # Inputs
/// - `x`: The X component (f32)
/// - `y`: The Y component (f32)
///
/// # Returns
/// A tuple (f32, f32) representing the 2D vector
///
/// # Example
/// If `x` is 3.0 and `y` is 4.0, the output will be (3.0, 4.0).
///
/// # Notes
/// Use this node to construct 2D points, directions, or velocities for further calculations.
/// # Make Vector2
/// Creates a 2D vector from X and Y components.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn make_vector2(x: f32, y: f32) -> (f32, f32) {
    (x, y)
}

/// Creates a new 2D vector from x and y components.
///
/// This node constructs a 2D vector (tuple of two numbers) from the provided x and y values.
///
/// # Inputs
/// - `x`: The x component (f32)
/// - `y`: The y component (f32)
///
/// # Returns
/// The resulting 2D vector as a tuple (f32, f32)
///
/// # Example
/// If `x` is 3.0 and `y` is 4.0, the output will be (3.0, 4.0).
///
/// # Notes
/// This node is useful for constructing 2D vectors for geometry, graphics, or math operations.
/// # Vector2 New
/// Creates a new 2D vector from x and y components.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector2_new(x: f32, y: f32) -> (f32, f32) {
    (x, y)
}

// =============================================================================
// Vector3 Construction
// =============================================================================

/// Creates a 3D vector from X, Y, and Z components.
///
/// This node constructs a 3D vector (tuple of three floats) from the provided X, Y, and Z components.
/// Useful for geometry, physics, graphics, and any context where 3D positions or directions are needed.
///
/// # Inputs
/// - `x`: The X component (f32)
/// - `y`: The Y component (f32)
/// - `z`: The Z component (f32)
///
/// # Returns
/// A tuple (f32, f32, f32) representing the 3D vector
///
/// # Example
/// If `x` is 1.0, `y` is 2.0, and `z` is 3.0, the output will be (1.0, 2.0, 3.0).
///
/// # Notes
/// Use this node to construct 3D points, directions, or colors (if using RGB).
/// # Make Vector3
/// Creates a 3D vector from X, Y, and Z components.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn make_vector3(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    (x, y, z)
}

/// Creates a new 3D vector from x, y, and z components.
///
/// This node constructs a 3D vector (tuple of three floating-point numbers) from the provided x, y, and z inputs.
///
/// # Inputs
/// - `x`: The x component (f32)
/// - `y`: The y component (f32)
/// - `z`: The z component (f32)
///
/// # Returns
/// The resulting 3D vector (f32, f32, f32)
///
/// # Example
/// If `x` is 1.0, `y` is 2.0, and `z` is 3.0, the output will be (1.0, 2.0, 3.0).
///
/// # Notes
/// This node is useful for constructing 3D vectors for use in graphics, physics, or spatial calculations.
/// # Vector3 New
/// Creates a new 3D vector from x, y, and z components.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector3_new(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    (x, y, z)
}

// =============================================================================
// Vector2 Deconstruction
// =============================================================================

/// Breaks a 2D vector into its X and Y components.
///
/// This node takes a 2D vector as input and returns its X and Y components as a tuple.
///
/// # Inputs
/// - `vector`: The input 2D vector (f32, f32)
///
/// # Returns
/// A tuple containing the X and Y components (f32, f32)
///
/// # Example
/// If `vector` is (3.0, 4.0), the output will be (3.0, 4.0).
///
/// # Notes
/// Useful for extracting individual coordinates from a vector.
/// # Break Vector2
/// Breaks a 2D vector into X and Y components.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn break_vector2(vector: (f32, f32)) -> (f32, f32) {
    let (x, y) = vector;
    (x, y)
}

// =============================================================================
// Vector3 Deconstruction
// =============================================================================

/// Breaks a 3D vector into its X, Y, and Z components.
///
/// This node takes a 3D vector as input and returns its individual X, Y, and Z values as a tuple.
///
/// # Inputs
/// - `vector`: The input 3D vector (f32, f32, f32)
///
/// # Returns
/// A tuple containing the X, Y, and Z components (f32, f32, f32)
///
/// # Example
/// If `vector` is (1.0, 2.0, 3.0), the output will be (1.0, 2.0, 3.0).
///
/// # Notes
/// Useful for extracting individual components from a 3D vector.
/// # Break Vector3
/// Breaks a 3D vector into X, Y, and Z components.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn break_vector3(vector: (f32, f32, f32)) -> (f32, f32, f32) {
    let (x, y, z) = vector;
    (x, y, z)
}

// =============================================================================
// Component Access
// =============================================================================

/// Gets the X component from a 2D or 3D vector.
///
/// This node extracts the X (horizontal) component from a vector input. For 2D vectors, it returns the first value.
///
/// # Inputs
/// - `vector`: The input vector (f32, f32)
///
/// # Returns
/// The X component (f32) of the input vector
///
/// # Example
/// If `vector` is (3.0, 4.0), the output will be 3.0.
///
/// # Notes
/// Use this node to access the horizontal coordinate of a vector for calculations, movement, or display.
/// # Get Vector X
/// Gets the X component from a 2D or 3D vector.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn get_vector_x(vector: (f32, f32)) -> f32 {
    let (x, _) = vector;
    x
}

/// Gets the Y component from a 2D or 3D vector.
///
/// This node extracts the Y component from the provided vector input.
/// Useful for geometry, physics, and vector math operations.
///
/// # Inputs
/// - `vector`: The input vector (f32, f32)
///
/// # Returns
/// The Y component (f32) of the input vector
///
/// # Example
/// If `vector` is (3.0, 4.0), the output will be 4.0.
///
/// # Notes
/// For 3D vectors, use only the first two components (x, y).
/// # Get Vector Y
/// Gets the Y component from a 2D or 3D vector.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn get_vector_y(vector: (f32, f32)) -> f32 {
    let (_, y) = vector;
    y
}

/// Gets the Z component from a 3D vector.
///
/// This node extracts the Z component from a 3D vector input.
///
/// # Inputs
/// - `vector`: The input 3D vector (f32, f32, f32)
///
/// # Returns
/// The Z component (f32) of the input vector
///
/// # Example
/// If `vector` is (1.0, 2.0, 3.0), the output will be 3.0.
///
/// # Notes
/// Useful for extracting the Z coordinate for spatial calculations, rendering, or physics.
/// # Get Vector Z
/// Gets the Z component from a 3D vector.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn get_vector_z(vector: (f32, f32, f32)) -> f32 {
    let (_, _, z) = vector;
    z
}

// =============================================================================
// Component Modification
// =============================================================================

/// Sets the X component of a 2D vector.
///
/// This node takes a 2D vector and a new X value, and returns a new vector with the X component replaced.
/// Useful for updating positions, modifying coordinates, or manipulating vector data in 2D space.
///
/// # Inputs
/// - `vector`: The input 2D vector (f32, f32)
/// - `new_x`: The new X value to set (f32)
///
/// # Returns
/// A new 2D vector (f32, f32) with the X component set to `new_x` and the Y component unchanged
///
/// # Example
/// If `vector` is (3.0, 4.0) and `new_x` is 7.0, the output will be (7.0, 4.0).
///
/// # Notes
/// Use this node to selectively update the X coordinate of a vector without affecting the Y coordinate.
/// # Set Vector X
/// Sets the X component of a 2D vector.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn set_vector_x(vector: (f32, f32), new_x: f32) -> (f32, f32) {
    let (_, y) = vector;
    (new_x, y)
}

/// Sets the Y component of a 2D vector.
///
/// This node takes a 2D vector and a new Y value, and returns a new vector with the same X component and the updated Y component.
/// Useful for modifying coordinates, updating positions, or manipulating vector data in 2D space.
///
/// # Inputs
/// - `vector`: The input 2D vector (f32, f32)
/// - `new_y`: The new Y component (f32)
///
/// # Returns
/// A tuple (f32, f32) representing the updated 2D vector
///
/// # Example
/// If `vector` is (3.0, 4.0) and `new_y` is 7.0, the output will be (3.0, 7.0).
///
/// # Notes
/// The X component is preserved from the input vector. Use this node to update only the Y value of a vector.
/// # Set Vector Y
/// Sets the Y component of a 2D vector.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn set_vector_y(vector: (f32, f32), new_y: f32) -> (f32, f32) {
    let (x, _) = vector;
    (x, new_y)
}

// =============================================================================
// Vector2 Arithmetic
// =============================================================================

/// Adds two 2D vectors component-wise.
///
/// This node takes two 2D vectors as input and returns a new 2D vector where each component
/// is the sum of the corresponding components of the input vectors.
///
/// # Inputs
/// - `a`: The first 2D vector (f32, f32)
/// - `b`: The second 2D vector (f32, f32)
///
/// # Returns
/// The resulting 2D vector after addition (f32, f32)
///
/// # Example
/// If `a` is (1.0, 2.0) and `b` is (3.0, 4.0), the output will be (4.0, 6.0).
///
/// # Notes
/// This operation is performed component-wise: (x1 + x2, y1 + y2).
/// # Vector2 Add
/// Adds two 2D vectors component-wise.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector2_add(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let (x1, y1) = a;
    let (x2, y2) = b;
    (x1 + x2, y1 + y2)
}

// =============================================================================
// Vector3 Arithmetic
// =============================================================================

/// Adds two 3D vectors component-wise.
///
/// This node takes two 3D vectors as input and returns a new 3D vector where each component
/// is the sum of the corresponding components of the input vectors.
///
/// # Inputs
/// - `a`: The first 3D vector (f32, f32, f32)
/// - `b`: The second 3D vector (f32, f32, f32)
///
/// # Returns
/// The resulting 3D vector after addition (f32, f32, f32)
///
/// # Example
/// If `a` is (1.0, 2.0, 3.0) and `b` is (4.0, 5.0, 6.0), the output will be (5.0, 7.0, 9.0).
///
/// # Notes
/// This operation is performed component-wise: (x1 + x2, y1 + y2, z1 + z2).
/// # Vector3 Add
/// Adds two 3D vectors component-wise.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector3_add(a: (f32, f32, f32), b: (f32, f32, f32)) -> (f32, f32, f32) {
    let (x1, y1, z1) = a;
    let (x2, y2, z2) = b;
    (x1 + x2, y1 + y2, z1 + z2)
}

// =============================================================================
// Vector2 Operations
// =============================================================================

/// Calculates the dot product of two 2D vectors.
///
/// This node computes the dot product of two input 2D vectors. The dot product is a scalar value
/// that is useful for determining the angle between vectors, projection, and other geometric calculations.
///
/// # Inputs
/// - `a`: The first 2D vector (f32, f32)
/// - `b`: The second 2D vector (f32, f32)
///
/// # Returns
/// The dot product of the two vectors (f32)
///
/// # Example
/// If `a` is (1.0, 2.0) and `b` is (3.0, 4.0), the output will be 1.0*3.0 + 2.0*4.0 = 11.0.
///
/// # Notes
/// The dot product is positive if the vectors point in a similar direction, negative if they point in opposite directions, and zero if they are perpendicular.
/// # Vector2 Dot
/// Calculates the dot product of two 2D vectors.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector2_dot(a: (f32, f32), b: (f32, f32)) -> f32 {
    let (x1, y1) = a;
    let (x2, y2) = b;
    x1 * x2 + y1 * y2
}

/// Calculates the magnitude (length) of a 2D vector.
///
/// This node computes the Euclidean length (magnitude) of a 2D vector using the formula `sqrt(x^2 + y^2)`.
///
/// # Inputs
/// - `vector`: The input 2D vector (f32, f32)
///
/// # Returns
/// The magnitude (length) of the vector (f32)
///
/// # Example
/// If `vector` is (3.0, 4.0), the output will be 5.0.
///
/// # Notes
/// The output is always non-negative. If the input vector is (0.0, 0.0), the output will be 0.0.
/// # Vector2 Magnitude
/// Calculates the magnitude (length) of a 2D vector.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector2_magnitude(vector: (f32, f32)) -> f32 {
    let (x, y) = vector;
    (x * x + y * y).sqrt()
}

/// Normalizes a 2D vector to unit length.
///
/// This node takes a 2D vector as input and returns a new vector in the same direction but with a magnitude (length) of 1.
/// If the input vector has zero length, the output will be (0.0, 0.0).
///
/// # Inputs
/// - `vector`: The input 2D vector (f32, f32)
///
/// # Returns
/// The normalized 2D vector (f32, f32)
///
/// # Example
/// If `vector` is (3.0, 4.0), the output will be (0.6, 0.8).
/// If `vector` is (0.0, 0.0), the output will be (0.0, 0.0).
///
/// # Notes
/// Normalization is useful for direction calculations and when you need a unit vector.
/// # Vector2 Normalize
/// Normalizes a 2D vector to unit length.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector2_normalize(vector: (f32, f32)) -> (f32, f32) {
    let (x, y) = vector;
    let magnitude = (x * x + y * y).sqrt();
    if magnitude > 0.0 {
        (x / magnitude, y / magnitude)
    } else {
        (0.0, 0.0)
    }
}

// =============================================================================
// Vector3 Operations
// =============================================================================

/// Calculates the dot product of two 3D vectors.
///
/// This node computes the dot product of two input 3D vectors. The dot product is a scalar value
/// that is useful for determining the angle between vectors, projection, and lighting calculations.
///
/// # Inputs
/// - `a`: The first 3D vector (f32, f32, f32)
/// - `b`: The second 3D vector (f32, f32, f32)
///
/// # Returns
/// The dot product of the two vectors (f32)
///
/// # Example
/// If `a` is (1.0, 2.0, 3.0) and `b` is (4.0, -5.0, 6.0), the output will be
/// 1.0*4.0 + 2.0*(-5.0) + 3.0*6.0 = 4.0 - 10.0 + 18.0 = 12.0
///
/// # Notes
/// The dot product is positive if the vectors point in a similar direction, negative if they point in opposite directions, and zero if they are perpendicular.
/// # Vector3 Dot
/// Calculates the dot product of two 3D vectors.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector3_dot(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
    let (x1, y1, z1) = a;
    let (x2, y2, z2) = b;
    x1 * x2 + y1 * y2 + z1 * z2
}

/// Calculates the cross product of two 3D vectors.
///
/// This node computes the cross product of two input 3D vectors, returning a new 3D vector
/// that is perpendicular to both input vectors. The cross product is commonly used in 3D graphics
/// and physics for calculating normals, torque, and rotational effects.
///
/// # Inputs
/// - `a`: The first 3D vector (f32, f32, f32)
/// - `b`: The second 3D vector (f32, f32, f32)
///
/// # Returns
/// The resulting 3D vector from the cross product (f32, f32, f32)
///
/// # Example
/// If `a` is (1.0, 0.0, 0.0) and `b` is (0.0, 1.0, 0.0), the output will be (0.0, 0.0, 1.0).
///
/// # Notes
/// The cross product is anti-commutative: swapping the inputs reverses the direction of the result.
/// # Vector3 Cross
/// Calculates the cross product of two 3D vectors.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector3_cross(a: (f32, f32, f32), b: (f32, f32, f32)) -> (f32, f32, f32) {
    let (x1, y1, z1) = a;
    let (x2, y2, z2) = b;
    (y1 * z2 - z1 * y2, z1 * x2 - x1 * z2, x1 * y2 - y1 * x2)
}

/// Calculates the magnitude (length) of a 3D vector.
///
/// This node computes the Euclidean length (magnitude) of a 3D vector using the formula
/// `sqrt(x^2 + y^2 + z^2)`.
///
/// # Inputs
/// - `vector`: The input vector (f32, f32, f32)
///
/// # Returns
/// The magnitude (length) of the vector (f32)
///
/// # Example
/// If `vector` is (3.0, 4.0, 12.0), the output will be 13.0.
///
/// # Notes
/// The output is always non-negative. If the input vector is (0.0, 0.0, 0.0), the output is 0.0.
/// # Vector3 Magnitude
/// Calculates the magnitude (length) of a 3D vector.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector3_magnitude(vector: (f32, f32, f32)) -> f32 {
    let (x, y, z) = vector;
    (x * x + y * y + z * z).sqrt()
}

/// Normalizes a 3D vector to unit length.
///
/// This node takes a 3D vector as input and returns a new vector in the same direction but with a magnitude (length) of 1.
/// If the input vector has zero length, the output will be the zero vector (0.0, 0.0, 0.0).
///
/// # Inputs
/// - `vector`: The input 3D vector (f32, f32, f32)
///
/// # Returns
/// The normalized 3D vector (f32, f32, f32)
///
/// # Example
/// If `vector` is (3.0, 0.0, 4.0), the output will be (0.6, 0.0, 0.8).
/// If `vector` is (0.0, 0.0, 0.0), the output will be (0.0, 0.0, 0.0).
///
/// # Notes
/// Normalization is useful for direction calculations and when you need a unit vector.
/// # Vector3 Normalize
/// Normalizes a 3D vector to unit length.
#[blueprint(type: NodeTypes::pure, category: "Vector", color: "#9013FE")]
pub fn vector3_normalize(vector: (f32, f32, f32)) -> (f32, f32, f32) {
    let (x, y, z) = vector;
    let magnitude = (x * x + y * y + z * z).sqrt();
    if magnitude > 0.0 {
        (x / magnitude, y / magnitude, z / magnitude)
    } else {
        (0.0, 0.0, 0.0)
    }
}
