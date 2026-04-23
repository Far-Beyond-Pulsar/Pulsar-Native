//! # Math Module
//!
//! Mathematical operations and functions for the Pulsar visual programming system.
//!
//! This module provides pure mathematical nodes including:
//! - Basic arithmetic (add, subtract, multiply, divide)
//! - Trigonometric functions (sin, cos, tan)
//! - Rounding and clamping operations
//! - Angular operations and conversions
//! - Distance calculations
//! - Interpolation and mapping functions
//! - Special mathematical utilities

use crate::blueprint;

// =============================================================================
// Basic Arithmetic Operations
// =============================================================================

/// Add two numbers together.
///
/// # Inputs
/// - `a`: First number (i64)
/// - `b`: Second number (i64)
///
/// # Returns
/// The sum of a and b
///
/// # Example
/// If `a` is 2 and `b` is 3, the output will be 5.
/// # Add
/// Adds two numbers together and returns the result.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Subtract one number from another.
///
/// # Inputs
/// - `a`: The minuend (the number to subtract from)
/// - `b`: The subtrahend (the number to subtract)
///
/// # Returns
/// The result of `a - b`
///
/// # Example
/// If `a` is 10 and `b` is 3, the output will be 7.
/// # Subtract
/// Subtracts b from a and returns the result.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn subtract(a: i64, b: i64) -> i64 {
    a - b
}

/// Multiply two numbers together.
///
/// # Inputs
/// - `a`: The first number
/// - `b`: The second number
///
/// # Returns
/// The product of the two input numbers
///
/// # Example
/// If `a` is 3 and `b` is 4, the output will be 12.
/// # Multiply
/// Multiplies two numbers together and returns the result.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn multiply(a: i64, b: i64) -> i64 {
    a * b
}

/// Divide one number by another.
///
/// # Inputs
/// - `a`: The dividend (numerator)
/// - `b`: The divisor (denominator)
///
/// # Returns
/// The result of `a / b`
///
/// # Example
/// If `a` is 10 and `b` is 2, the output will be 5.
/// If `b` is 0, the output will be infinity or NaN.
///
/// # Notes
/// Division by zero is not checked; handle with care if the divisor may be zero.
/// # Divide
/// Divides a by b and returns the result.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn divide(a: i64, b: i64) -> i64 {
    if b == 0 {
        0
    } else {
        a / b
    }
}

/// Returns the remainder of division (modulo operation).
///
/// # Inputs
/// - `a`: The dividend (number to be divided)
/// - `b`: The divisor (number to divide by)
///
/// # Returns
/// The remainder of the division (`a` % `b`)
///
/// # Example
/// If `a` is 10 and `b` is 3, the output will be 1.
/// If `a` is 7 and `b` is 2, the output will be 1.
///
/// # Notes
/// The result has the same sign as the dividend. Division by zero will panic or return NaN depending on the type.
/// # Modulo
/// Returns the remainder of division (a % b).
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn modulo(a: i64, b: i64) -> i64 {
    a % b
}

// =============================================================================
// Absolute Value and Sign
// =============================================================================

/// Returns the absolute value of a number.
///
/// # Inputs
/// - `value`: The input number
///
/// # Returns
/// The absolute value of the input number
///
/// # Example
/// If `value` is -5, the output will be 5.
/// # Abs
/// Returns the absolute value of a number.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn abs(value: f64) -> f64 {
    value.abs()
}

/// Returns the sign of a number (-1, 0, or 1).
///
/// # Inputs
/// - `value`: The input number (float)
///
/// # Returns
/// The sign of the input number as a float: 1.0, -1.0, or 0.0
///
/// # Example
/// If `value` is 7.5, the output will be 1.0.
/// If `value` is -3.2, the output will be -1.0.
/// If `value` is 0.0, the output will be 0.0.
///
/// # Notes
/// The output is always a float. Use this node to extract the direction or polarity of a value.
/// # Sign
/// Returns the sign of a number: 1.0 for positive, -1.0 for negative, 0.0 for zero.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn sign(value: f64) -> f64 {
    value.signum()
}

// =============================================================================
// Rounding Operations
// =============================================================================

/// Returns the smallest integer greater than or equal to a number.
///
/// # Inputs
/// - `value`: The input number (float)
///
/// # Returns
/// The smallest integer greater than or equal to the input (float)
///
/// # Example
/// If `value` is 3.2, the output will be 4.0.
/// If `value` is -2.7, the output will be -2.0.
///
/// # Notes
/// The output is a float, not an integer type.
/// # Ceil
/// Returns the smallest integer greater than or equal to a number (ceiling).
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn ceil(value: f64) -> f64 {
    value.ceil()
}

/// Returns the largest integer less than or equal to a number.
///
/// # Inputs
/// - `value`: The input number (float)
///
/// # Returns
/// The largest integer less than or equal to the input (float)
///
/// # Example
/// If `value` is 3.7, the output will be 3.0.
/// If `value` is -2.3, the output will be -3.0.
///
/// # Notes
/// The output is a float, not an integer type.
/// # Floor
/// Returns the largest integer less than or equal to a number (floor).
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn floor(value: f64) -> f64 {
    value.floor()
}

/// Rounds a number to the nearest integer.
///
/// # Inputs
/// - `value`: The input number (float) to round
///
/// # Returns
/// The rounded value as a float
///
/// # Example
/// If `value` is 3.6, the output will be 4.0.
/// If `value` is -2.3, the output will be -2.0.
///
/// # Notes
/// The output is a float, not an integer type. Rounds half-way cases away from zero (e.g., 2.5 -> 3.0, -2.5 -> -3.0).
/// # Round
/// Rounds a number to the nearest integer.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn round(value: f64) -> f64 {
    value.round()
}

// =============================================================================
// Power and Root Operations
// =============================================================================

/// Raises a number to a power.
///
/// # Inputs
/// - `base`: The base number
/// - `exponent`: The exponent to raise the base to
///
/// # Returns
/// The result of `base` raised to the power of `exponent`
///
/// # Example
/// If `base` is 2 and `exponent` is 3, the output will be 8.
/// If `base` is 9 and `exponent` is 0.5, the output will be 3 (square root).
///
/// # Notes
/// Uses the standard floating-point powf function. Negative bases and non-integer exponents may yield complex results (NaN).
/// # Power
/// Raises a number to a power (base^exponent).
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn power(base: f64, exponent: f64) -> f64 {
    base.powf(exponent)
}

/// Calculates the square root of a number.
///
/// # Inputs
/// - `value`: The input number (float) to calculate the square root of
///
/// # Returns
/// The square root of the input number (float)
///
/// # Example
/// If `value` is 9.0, the output will be 3.0.
/// If `value` is 2.0, the output will be approximately 1.4142.
///
/// # Notes
/// The input must be non-negative. Negative inputs will return NaN (not a number).
/// Use this node for root calculations, distance formulas, or normalization.
/// # Square Root
/// Calculates the square root of a number.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn sqrt(value: f64) -> f64 {
    value.sqrt()
}

// =============================================================================
// Trigonometric Functions
// =============================================================================

/// Calculates the sine of an angle in radians.
///
/// # Inputs
/// - `angle`: The angle in radians
///
/// # Returns
/// The sine of the input angle (float)
///
/// # Example
/// If `angle` is 0.0, the output will be 0.0.
/// If `angle` is 3.14159 / 2 (π/2), the output will be approximately 1.0.
///
/// # Notes
/// The input should be in radians. For degrees, convert to radians before using this node.
/// # Sin
/// Calculates the sine of an angle in radians.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn sin(angle: f64) -> f64 {
    angle.sin()
}

/// Calculates the cosine of an angle in radians.
///
/// # Inputs
/// - `angle`: The angle in radians
///
/// # Returns
/// The cosine of the input angle (float)
///
/// # Example
/// If `angle` is 0.0, the output will be 1.0.
///
/// # Notes
/// The input should be in radians. For degrees, convert to radians before using this node.
/// # Cos
/// Calculates the cosine of an angle in radians.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn cos(angle: f64) -> f64 {
    angle.cos()
}

/// Calculates the tangent of an angle in radians.
///
/// # Inputs
/// - `angle`: The angle in radians (floating point number)
///
/// # Returns
/// The tangent of the input angle (floating point number)
///
/// # Example
/// If `angle` is `0.0`, the output will be `0.0`.
/// If `angle` is π/4, the output will be approximately `1.0`.
///
/// # Notes
/// The tangent function has asymptotes at odd multiples of π/2. Inputting such values will result in very large outputs or `NaN`.
/// # Tan
/// Calculates the tangent of an angle in radians.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn tan(angle: f64) -> f64 {
    angle.tan()
}

// =============================================================================
// Angular Operations
// =============================================================================

/// Converts degrees to radians.
///
/// # Inputs
/// - `degrees`: The angle in degrees
///
/// # Returns
/// The angle converted to radians (float)
///
/// # Example
/// If `degrees` is 180.0, the output will be approximately 3.14159 (π).
///
/// # Notes
/// Use this node when trigonometric functions require radians instead of degrees.
/// # Degrees to Radians
/// Converts an angle from degrees to radians.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn degrees_to_radians(degrees: f64) -> f64 {
    degrees.to_radians()
}

/// Converts an angle from radians to degrees.
///
/// # Inputs
/// - `radians`: The angle in radians
///
/// # Returns
/// The angle converted to degrees (float)
///
/// # Example
/// If `radians` is 3.14159 (π), the output will be approximately 180.0.
///
/// # Notes
/// Use this node when you need to convert angles for display, user input, or APIs that expect degrees.
/// # Radians to Degrees
/// Converts an angle from radians to degrees.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn radians_to_degrees(radians: f64) -> f64 {
    radians.to_degrees()
}

/// Calculates the shortest angular difference between two angles.
///
/// # Inputs
/// - `angle1`: The first angle (in radians)
/// - `angle2`: The second angle (in radians)
///
/// # Returns
/// The shortest angular difference (in radians), signed
///
/// # Example
/// If angle1 = 0.0 and angle2 = π, the output will be π.
/// If angle1 = π and angle2 = 0.0, the output will be -π.
///
/// # Notes
/// The result is always the minimal difference, taking wrap-around into account.
/// # Angle Difference
/// Calculates the shortest angular difference between two angles.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn angle_difference(angle1: f64, angle2: f64) -> f64 {
    let diff = angle2 - angle1;
    let pi = std::f64::consts::PI;
    ((diff + pi) % (2.0 * pi)) - pi
}

// =============================================================================
// Min/Max Operations
// =============================================================================

/// Returns the minimum of two numbers.
///
/// # Inputs
/// - `a`: The first number
/// - `b`: The second number
///
/// # Returns
/// The minimum of the two input numbers
///
/// # Example
/// If `a` is 5 and `b` is 3, the output will be 3.
/// If both are equal, the output will be that value.
///
/// # Notes
/// Uses the standard min function for the numeric type.
/// # Min
/// Returns the minimum of two numbers.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn min(a: f64, b: f64) -> f64 {
    a.min(b)
}

/// Returns the maximum of two numbers.
///
/// # Inputs
/// - `a`: The first number
/// - `b`: The second number
///
/// # Returns
/// The maximum of the two input numbers
///
/// # Example
/// If `a` is 5 and `b` is 8, the output will be 8.
/// If both are equal, the output will be that value.
///
/// # Notes
/// Uses the standard max function for the numeric type.
/// # Max
/// Returns the maximum of two numbers.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn max(a: f64, b: f64) -> f64 {
    a.max(b)
}

// =============================================================================
// Clamping and Range Operations
// =============================================================================

/// Clamps a value between a minimum and maximum.
///
/// # Inputs
/// - `value`: The value to clamp
/// - `min`: The minimum bound
/// - `max`: The maximum bound
///
/// # Returns
/// The clamped value, guaranteed to be between `min` and `max`
///
/// # Example
/// If `value` is 15, `min` is 10, and `max` is 12, the output will be 12.
///
/// # Notes
/// Useful for enforcing numeric constraints and preventing out-of-range values.
/// # Clamp
/// Clamps a value between a minimum and maximum.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.clamp(min, max)
}

/// Clamps a value to a specific numeric range.
///
/// # Inputs
/// - `value`: The value to clamp
/// - `min`: The minimum bound
/// - `max`: The maximum bound
///
/// # Returns
/// The clamped value (f32)
///
/// # Example
/// If `value` is 10, `min` is 0, and `max` is 5, the output will be 5.
///
/// # Notes
/// Useful for restricting values to a valid range, such as for UI sliders, physics, or normalization.
/// # Clamp to Range
/// Clamps a value to a specified range [min, max].
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn clamp_to_range(value: f32, min: f32, max: f32) -> f32 {
    value.clamp(min, max)
}

/// Checks if a value is within a specified numeric range (inclusive).
///
/// # Inputs
/// - `value`: The value to check
/// - `min`: The minimum bound (inclusive)
/// - `max`: The maximum bound (inclusive)
///
/// # Returns
/// Returns `true` if the value is within the range [min, max], `false` otherwise
///
/// # Example
/// If `value` is 5, `min` is 3, and `max` is 7, the output will be true.
/// If `value` is 2, the output will be false.
///
/// # Notes
/// The check is inclusive: value >= min && value <= max.
/// # In Range
/// Checks if a value is within a specified range (inclusive).
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn in_range(value: f32, min: f32, max: f32) -> bool {
    value >= min && value <= max
}

// =============================================================================
// Interpolation and Mapping
// =============================================================================

/// Linearly interpolates between two values (lerp).
///
/// # Inputs
/// - `a`: The start value (a)
/// - `b`: The end value (b)
/// - `t`: The interpolation parameter (float, typically in [0, 1])
///
/// # Returns
/// The interpolated value between a and b
///
/// # Example
/// If `a` is 0, `b` is 10, and `t` is 0.5, the output will be 5.0.
/// If `t` is 0, the output is a. If `t` is 1, the output is b.
///
/// # Notes
/// The parameter t is not clamped; values outside [0, 1] will extrapolate beyond a and b.
/// # Lerp
/// Linearly interpolates between two values using parameter t.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Maps a value from one numeric range to another.
///
/// # Inputs
/// - `value`: The value to map
/// - `from_min`: The minimum of the input range
/// - `from_max`: The maximum of the input range
/// - `to_min`: The minimum of the output range
/// - `to_max`: The maximum of the output range
///
/// # Returns
/// The mapped value in the output range
///
/// # Example
/// If `value` is 5, `from_min` is 0, `from_max` is 10, `to_min` is 0, and `to_max` is 100, the output will be 50.
///
/// # Notes
/// If the input range is zero (from_min == from_max), the output will be to_min. No clamping is performed on the output.
/// # Map Range
/// Maps a value from one range to another.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn map_range(value: f32, from_min: f32, from_max: f32, to_min: f32, to_max: f32) -> f32 {
    if from_max == from_min {
        to_min
    } else {
        to_min + (value - from_min) * (to_max - to_min) / (from_max - from_min)
    }
}

/// Performs smooth interpolation between two edges (smoothstep).
///
/// # Inputs
/// - `edge0`: The lower edge of the transition (float)
/// - `edge1`: The upper edge of the transition (float)
/// - `x`: The input value to interpolate (float)
///
/// # Returns
/// A float in the range [0, 1] representing the smooth interpolation between the edges
///
/// # Example
/// If `edge0` is 0.0, `edge1` is 1.0, and `x` is 0.5, the output will be 0.5.
/// If `x` is less than 0.0, the output will be 0.0. If greater than 1.0, the output will be 1.0.
///
/// # Notes
/// The formula used is: t * t * (3 - 2 * t), where t = ((x - edge0) / (edge1 - edge0)) clamped to [0, 1].
/// Use this node for smooth transitions, anti-aliasing, or soft step functions.
/// # Smoothstep
/// Performs smooth Hermite interpolation between two edges.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// =============================================================================
// Distance Calculations
// =============================================================================

/// Calculates the distance between two 2D points.
///
/// # Inputs
/// - `a`: The first point as a tuple (x1, y1)
/// - `b`: The second point as a tuple (x2, y2)
///
/// # Returns
/// The distance between the two points (float)
///
/// # Example
/// If `a` is (0.0, 0.0) and `b` is (3.0, 4.0), the output will be 5.0.
///
/// # Notes
/// Useful for geometry, physics, and spatial calculations.
/// # Distance 2D
/// Calculates the Euclidean distance between two 2D points.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn distance2d(a: (f32, f32), b: (f32, f32)) -> f32 {
    let (x1, y1) = a;
    let (x2, y2) = b;
    let dx = x2 - x1;
    let dy = y2 - y1;
    (dx * dx + dy * dy).sqrt()
}

/// Calculates the Euclidean distance between two 3D points.
///
/// # Inputs
/// - `a`: The first 3D point (tuple of three floats: x, y, z)
/// - `b`: The second 3D point (tuple of three floats: x, y, z)
///
/// # Returns
/// The distance between the two points (float)
///
/// # Example
/// If `a` is (0.0, 0.0, 0.0) and `b` is (1.0, 2.0, 2.0), the output will be 3.0.
///
/// # Notes
/// Useful for geometry, physics, and spatial calculations.
/// # Distance 3D
/// Calculates the Euclidean distance between two 3D points.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn distance3d(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
    let (x1, y1, z1) = a;
    let (x2, y2, z2) = b;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let dz = z2 - z1;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

// =============================================================================
// Special Mathematical Utilities
// =============================================================================

/// Creates a bouncing value that oscillates between 0 and 1.
///
/// # Inputs
/// - `time`: The current time value (float)
/// - `frequency`: The frequency of the bounce (float)
///
/// # Returns
/// A float value in the range [0, 1] representing the bounce
///
/// # Example
/// If `time` increases steadily and `frequency` is 1.0,
/// the output will oscillate between 0 and 1 in a smooth, periodic fashion.
///
/// # Notes
/// Useful for animations, effects, or periodic triggers.
/// # Bounce Value
/// Creates a bouncing value that oscillates between 0 and 1.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn bounce_value(time: f32, frequency: f32) -> f32 {
    (time * frequency).sin().abs()
}

/// Creates a ping-pong value that bounces between 0 and a specified length.
///
/// # Inputs
/// - `t`: The input time or parameter (float)
/// - `length`: The maximum value (float) to bounce between
///
/// # Returns
/// A float value that smoothly bounces between 0 and `length`
///
/// # Example
/// If `t` increases steadily and `length` is 3.0, the output will go from 0 to 3, then 3 to 0, and repeat.
///
/// # Notes
/// The output is periodic and continuous. The period is twice the length. Useful for looping animations or oscillating values.
/// # Ping Pong
/// Creates a ping-pong value that bounces between 0 and length.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn ping_pong(t: f32, length: f32) -> f32 {
    let cycle = (t / length).floor();
    let position = t % length;
    if cycle as i32 % 2 == 0 {
        position
    } else {
        length - position
    }
}

// =============================================================================
// Bitwise Operations
// =============================================================================

/// Bitwise AND operation.
///
/// # Inputs
/// - `a`: First integer
/// - `b`: Second integer
///
/// # Returns
/// Bitwise AND result
///
/// # Math Bitwise AND
/// Performs bitwise AND operation on two integers.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn bitwise_and(a: i64, b: i64) -> i64 {
    a & b
}

/// Bitwise OR operation.
///
/// # Inputs
/// - `a`: First integer
/// - `b`: Second integer
///
/// # Returns
/// Bitwise OR result
///
/// # Math Bitwise OR
/// Performs bitwise OR operation on two integers.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn bitwise_or(a: i64, b: i64) -> i64 {
    a | b
}

/// Bitwise XOR operation.
///
/// # Inputs
/// - `a`: First integer
/// - `b`: Second integer
///
/// # Returns
/// Bitwise XOR result
///
/// # Math Bitwise XOR
/// Performs bitwise XOR operation on two integers.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn bitwise_xor(a: i64, b: i64) -> i64 {
    a ^ b
}

/// Bitwise NOT operation.
///
/// # Inputs
/// - `value`: The integer to invert
///
/// # Returns
/// Bitwise NOT result
///
/// # Math Bitwise NOT
/// Performs bitwise NOT operation (bit inversion).
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn bitwise_not(value: i64) -> i64 {
    !value
}

/// Left bit shift operation.
///
/// # Inputs
/// - `value`: The value to shift
/// - `bits`: Number of bits to shift
///
/// # Returns
/// Left-shifted result
///
/// # Math Bit Shift Left
/// Shifts bits to the left.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn bit_shift_left(value: i64, bits: i64) -> i64 {
    value << bits.clamp(0, 63)
}

/// Right bit shift operation.
///
/// # Inputs
/// - `value`: The value to shift
/// - `bits`: Number of bits to shift
///
/// # Returns
/// Right-shifted result
///
/// # Math Bit Shift Right
/// Shifts bits to the right.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn bit_shift_right(value: i64, bits: i64) -> i64 {
    value >> bits.clamp(0, 63)
}

/// Count the number of set bits (population count).
///
/// # Inputs
/// - `value`: The integer to analyze
///
/// # Returns
/// Number of 1 bits
///
/// # Math Count Bits
/// Counts the number of set bits (1s) in an integer.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn count_bits(value: i64) -> i64 {
    value.count_ones() as i64
}

/// Get a specific bit from an integer.
///
/// # Inputs
/// - `value`: The integer
/// - `bit_index`: The bit position (0 = least significant)
///
/// # Returns
/// 1 if bit is set, 0 otherwise
///
/// # Math Get Bit
/// Gets the value of a specific bit.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn get_bit(value: i64, bit_index: i64) -> i64 {
    if bit_index < 0 || bit_index >= 64 {
        return 0;
    }
    (value >> bit_index) & 1
}

/// Set a specific bit in an integer.
///
/// # Inputs
/// - `value`: The integer
/// - `bit_index`: The bit position (0 = least significant)
///
/// # Returns
/// Integer with the specified bit set
///
/// # Math Set Bit
/// Sets a specific bit to 1.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn set_bit(value: i64, bit_index: i64) -> i64 {
    if bit_index < 0 || bit_index >= 64 {
        return value;
    }
    value | (1 << bit_index)
}

/// Clear a specific bit in an integer.
///
/// # Inputs
/// - `value`: The integer
/// - `bit_index`: The bit position (0 = least significant)
///
/// # Returns
/// Integer with the specified bit cleared
///
/// # Math Clear Bit
/// Clears a specific bit to 0.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn clear_bit(value: i64, bit_index: i64) -> i64 {
    if bit_index < 0 || bit_index >= 64 {
        return value;
    }
    value & !(1 << bit_index)
}

/// Toggle a specific bit in an integer.
///
/// # Inputs
/// - `value`: The integer
/// - `bit_index`: The bit position (0 = least significant)
///
/// # Returns
/// Integer with the specified bit toggled
///
/// # Math Toggle Bit
/// Toggles a specific bit (0->1 or 1->0).
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn toggle_bit(value: i64, bit_index: i64) -> i64 {
    if bit_index < 0 || bit_index >= 64 {
        return value;
    }
    value ^ (1 << bit_index)
}

// =============================================================================
// Statistical Operations
// =============================================================================

/// Calculate the mean (average) of two values.
///
/// # Inputs
/// - `a`: First value
/// - `b`: Second value
///
/// # Returns
/// The mean of the two values
///
/// # Math Mean
/// Calculates the mean (average) of two values.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn mean(a: f64, b: f64) -> f64 {
    (a + b) / 2.0
}

/// Calculate the median of three values.
///
/// # Inputs
/// - `a`: First value
/// - `b`: Second value
/// - `c`: Third value
///
/// # Returns
/// The median value
///
/// # Math Median
/// Finds the median (middle value) of three numbers.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn median(a: f64, b: f64, c: f64) -> f64 {
    let mut values = [a, b, c];
    values.sort_by(|x, y| x.partial_cmp(y).unwrap());
    values[1]
}

/// Calculate variance of two values from their mean.
///
/// # Inputs
/// - `a`: First value
/// - `b`: Second value
///
/// # Returns
/// The variance
///
/// # Math Variance
/// Calculates variance of two values.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn variance(a: f64, b: f64) -> f64 {
    let mean_val = (a + b) / 2.0;
    let diff_a = a - mean_val;
    let diff_b = b - mean_val;
    (diff_a * diff_a + diff_b * diff_b) / 2.0
}

/// Calculate standard deviation of two values.
///
/// # Inputs
/// - `a`: First value
/// - `b`: Second value
///
/// # Returns
/// The standard deviation
///
/// # Math Standard Deviation
/// Calculates standard deviation of two values.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn std_dev(a: f64, b: f64) -> f64 {
    variance(a, b).sqrt()
}

/// Calculate the range (difference between max and min).
///
/// # Inputs
/// - `a`: First value
/// - `b`: Second value
///
/// # Returns
/// The range (absolute difference)
///
/// # Math Range
/// Calculates the range (difference) between two values.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn range(a: f64, b: f64) -> f64 {
    (a - b).abs()
}

/// Normalize a value to 0-1 range.
///
/// # Inputs
/// - `value`: The value to normalize
/// - `min`: The minimum of the range
/// - `max`: The maximum of the range
///
/// # Returns
/// Normalized value (0-1)
///
/// # Math Normalize
/// Normalizes a value to the 0-1 range.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn normalize(value: f64, min: f64, max: f64) -> f64 {
    if max == min {
        return 0.0;
    }
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

/// Denormalize a 0-1 value to a range.
///
/// # Inputs
/// - `normalized`: The normalized value (0-1)
/// - `min`: The minimum of the target range
/// - `max`: The maximum of the target range
///
/// # Returns
/// Denormalized value
///
/// # Math Denormalize
/// Denormalizes a 0-1 value to a specific range.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn denormalize(normalized: f64, min: f64, max: f64) -> f64 {
    min + normalized.clamp(0.0, 1.0) * (max - min)
}

/// Calculate percentage of a value relative to a total.
///
/// # Inputs
/// - `value`: The part value
/// - `total`: The total value
///
/// # Returns
/// Percentage (0-100)
///
/// # Math Percentage
/// Calculates what percentage value is of total.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn percentage(value: f64, total: f64) -> f64 {
    if total == 0.0 {
        return 0.0;
    }
    (value / total) * 100.0
}

/// Calculate value from percentage of total.
///
/// # Inputs
/// - `percentage`: The percentage (0-100)
/// - `total`: The total value
///
/// # Returns
/// The calculated value
///
/// # Math From Percentage
/// Calculates a value from a percentage of a total.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn from_percentage(percentage: f64, total: f64) -> f64 {
    (percentage / 100.0) * total
}

/// Calculate ratio between two values.
///
/// # Inputs
/// - `a`: First value
/// - `b`: Second value
///
/// # Returns
/// The ratio a/b
///
/// # Math Ratio
/// Calculates the ratio between two values.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn ratio(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        return 0.0;
    }
    a / b
}

/// Calculate proportion (part/whole).
///
/// # Inputs
/// - `part`: The part value
/// - `whole`: The whole value
///
/// # Returns
/// The proportion (0-1)
///
/// # Math Proportion
/// Calculates the proportion of part to whole.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn proportion(part: f64, whole: f64) -> f64 {
    if whole == 0.0 {
        return 0.0;
    }
    (part / whole).clamp(0.0, 1.0)
}
