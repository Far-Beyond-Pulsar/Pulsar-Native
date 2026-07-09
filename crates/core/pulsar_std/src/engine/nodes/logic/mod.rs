//! # Logic Module
//!
//! Boolean logic and comparison operations for the Pulsar visual programming system.
//!
//! This module provides pure logic nodes including:
//! - Boolean operations (AND, OR, NOT)
//! - Comparison operations (equals, not equals, greater than, less than, etc.)
//! - Range checking (is_between)
//! - Approximate equality (nearly_equal)
//! - Conditional selection (select_bool, select_number, select_string)

use crate::blueprint;

// =============================================================================
// Boolean Logic Operations
// =============================================================================

/// Performs a logical AND operation between two boolean values.
///
/// # Inputs
/// - `a`: The first boolean value
/// - `b`: The second boolean value
///
/// # Returns
/// The result of logical AND between the two input booleans (true if both are true, false otherwise)
///
/// # Example
/// If `a` is true and `b` is false, the output will be false.
/// # And
/// Performs a logical AND operation between two boolean values.
/// Returns true if both inputs are true, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn and(a: bool, b: bool) -> bool {
    a && b
}

/// Performs a logical OR operation between two boolean values.
///
/// This node takes two boolean inputs and returns true if either input is true, or both are true.
/// Useful for conditional logic, branching, and combining multiple conditions.
///
/// # Inputs
/// - `a`: The first boolean value
/// - `b`: The second boolean value
///
/// # Returns
/// Returns `true` if either input is true, `false` otherwise
///
/// # Example
/// If `a` is true and `b` is false, the output will be true.
/// If both are false, the output will be false.
///
/// # Notes
/// Logical OR returns true if at least one input is true. Use for combining conditions in control flow.
/// # Or
/// Performs a logical OR operation between two boolean values.
/// Returns true if either input is true, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn or(a: bool, b: bool) -> bool {
    a || b
}

/// Performs a logical NOT operation on a boolean value.
///
/// This node takes a boolean input and returns its logical negation (true becomes false, false becomes true).
/// Useful for inverting conditions, toggling states, or controlling flow based on the opposite of a condition.
///
/// # Inputs
/// - `value`: The boolean value to negate
///
/// # Returns
/// The logical NOT of the input value (true if input is false, false if input is true)
///
/// # Example
/// If `value` is true, the output will be false. If `value` is false, the output will be true.
///
/// # Notes
/// Use this node to invert conditions in branches, filters, or toggles.
/// # Not
/// Performs a logical NOT operation on a boolean value.
/// Returns the opposite boolean value.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn not(value: bool) -> bool {
    !value
}

// =============================================================================
// Comparison Operations (Generic)
// =============================================================================

/// Checks if two values are equal.
///
/// This node compares two input values and returns true if they are equal, false otherwise.
/// Works for numbers, strings, booleans, and other comparable types.
///
/// # Inputs
/// - `a`: The first value to compare (i64)
/// - `b`: The second value to compare (i64)
///
/// # Returns
/// Returns `true` if the values are equal, `false` otherwise
///
/// # Example
/// If `a` is 5 and `b` is 5, the output will be true.
/// If `a` is 3 and `b` is 7, the output will be false.
///
/// # Notes
/// Comparison uses the standard equality operator. For custom types, ensure they implement equality.
/// # Equals
/// Checks if two values are equal.
/// Returns true if the values are equal, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn equals(a: i64, b: i64) -> bool {
    a == b
}

/// Checks if two values are not equal.
///
/// This node compares two input values and returns true if they are not equal, false otherwise.
/// Works for numbers, strings, booleans, and other comparable types.
///
/// # Inputs
/// - `a`: The first value to compare (i64)
/// - `b`: The second value to compare (i64)
///
/// # Returns
/// Returns `true` if the values are not equal, `false` otherwise
///
/// # Example
/// If `a` is 5 and `b` is 3, the output will be true.
/// If `a` is 7 and `b` is 7, the output will be false.
///
/// # Notes
/// Comparison uses the standard inequality operator. For custom types, ensure they implement equality/inequality.
/// # Not Equals
/// Checks if two values are not equal.
/// Returns true if the values are not equal, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn not_equals(a: i64, b: i64) -> bool {
    a != b
}

// =============================================================================
// Numeric Comparison Operations
// =============================================================================

/// Checks if one number is greater than another.
///
/// This node compares two numeric inputs and returns true if the first input is greater than the second, false otherwise.
///
/// # Inputs
/// - `a`: The first number to compare (i64)
/// - `b`: The second number to compare (i64)
///
/// # Returns
/// Returns `true` if `a` is greater than `b`, `false` otherwise
///
/// # Example
/// If `a` is 7 and `b` is 3, the output will be true.
/// If `a` is 2 and `b` is 5, the output will be false.
///
/// # Notes
/// Useful for conditional logic, filtering, and comparisons in your graph.
/// # Greater Than
/// Checks if one number is greater than another.
/// Returns true if a > b, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn greater_than(a: i64, b: i64) -> bool {
    a > b
}

/// Checks if one number is less than another.
///
/// This node compares two numeric inputs and returns true if the first input is less than the second, false otherwise.
/// Useful for conditional logic, filtering, and comparisons in your graph.
///
/// # Inputs
/// - `a`: The first number to compare (i64)
/// - `b`: The second number to compare (i64)
///
/// # Returns
/// Returns `true` if `a` is less than `b`, `false` otherwise
///
/// # Example
/// If `a` is 3 and `b` is 7, the output will be true.
/// If `a` is 5 and `b` is 2, the output will be false.
///
/// # Notes
/// Useful for branching, validation, and enforcing numeric constraints.
/// # Less Than
/// Checks if one number is less than another.
/// Returns true if a < b, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn less_than(a: i64, b: i64) -> bool {
    a < b
}

/// Checks if one number is greater than or equal to another.
///
/// This node compares two numeric inputs and returns true if the first input is greater than or equal to the second, false otherwise.
///
/// # Inputs
/// - `a`: The first number to compare (i64)
/// - `b`: The second number to compare (i64)
///
/// # Returns
/// Returns `true` if `a` is greater than or equal to `b`, `false` otherwise
///
/// # Example
/// If `a` is 5 and `b` is 3, the output will be true.
/// If `a` is 2 and `b` is 2, the output will be true.
/// If `a` is 1 and `b` is 4, the output will be false.
///
/// # Notes
/// Useful for conditional logic, filtering, and comparisons in control flow.
/// # Greater Equal
/// Checks if one number is greater than or equal to another.
/// Returns true if a >= b, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn greater_equal(a: i64, b: i64) -> bool {
    a >= b
}

/// Checks if one number is less than or equal to another.
///
/// This node compares two numeric inputs and returns true if the first input is less than or equal to the second, false otherwise.
/// Useful for conditional logic, filtering, and comparisons in control flow.
///
/// # Inputs
/// - `a`: The first number to compare (i64)
/// - `b`: The second number to compare (i64)
///
/// # Returns
/// Returns `true` if `a` is less than or equal to `b`, `false` otherwise
///
/// # Example
/// If `a` is 3 and `b` is 5, the output will be true.
/// If `a` is 7 and `b` is 7, the output will be true.
/// If `a` is 9 and `b` is 2, the output will be false.
///
/// # Notes
/// Useful for range checks, validation, and branching logic.
/// # Less Equal
/// Checks if one number is less than or equal to another.
/// Returns true if a <= b, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn less_equal(a: i64, b: i64) -> bool {
    a <= b
}

// =============================================================================
// Range and Proximity Checks
// =============================================================================

/// Checks if a value is strictly between two other values (exclusive).
///
/// This node returns true if the input value is greater than the first bound and less than the second bound (exclusive).
/// Useful for range validation, filtering, and enforcing exclusive constraints.
///
/// # Inputs
/// - `value`: The value to check (f64)
/// - `a`: The lower bound (exclusive, f64)
/// - `b`: The upper bound (exclusive, f64)
///
/// # Returns
/// Returns `true` if the value is strictly between a and b (a < value < b), `false` otherwise
///
/// # Example
/// If `value` is 5.0, `a` is 3.0, and `b` is 7.0, the output will be true.
/// If `value` is 3.0 or 7.0, the output will be false.
///
/// # Notes
/// The check is exclusive: value > a && value < b.
/// # Is Between
/// Checks if a value is strictly between two other values (exclusive).
/// Returns true if a < value < b, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn is_between(value: f64, a: f64, b: f64) -> bool {
    value > a && value < b
}

/// Checks if two numbers are nearly equal within a tolerance.
///
/// This function performs an approximate equality check, useful when comparing floating-point numbers
/// where exact equality may not be achievable due to rounding errors.
///
/// # Inputs
/// - `a`: The first number (f64)
/// - `b`: The second number (f64)
/// - `tolerance`: The maximum allowed difference between a and b (f64)
///
/// # Returns
/// Returns `true` if the absolute difference between a and b is less than or equal to tolerance, `false` otherwise
///
/// # Example
/// If `a` is 1.0001, `b` is 1.0, and `tolerance` is 0.001, the output will be true.
/// If `tolerance` is 0.00001, the output will be false.
///
/// # Notes
/// Useful for floating-point comparisons where exact equality is not reliable.
/// # Nearly Equal
/// Checks if two numbers are nearly equal within a tolerance.
/// Returns true if |a - b| <= tolerance, false otherwise.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn nearly_equal(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

// =============================================================================
// Conditional Selection (Ternary Operations)
// =============================================================================

/// Selects between two boolean values based on a condition (ternary operator).
///
/// This node takes a boolean condition and two boolean values, returning `a` if the condition is true, or `b` if the condition is false.
/// Useful for conditional logic, branching, and simplifying if-else expressions in your graph.
///
/// # Inputs
/// - `condition`: The condition to evaluate (bool)
/// - `a`: The value to return if the condition is true (bool)
/// - `b`: The value to return if the condition is false (bool)
///
/// # Returns
/// The selected boolean value based on the condition
///
/// # Example
/// If `condition` is true, the output will be `a`. If false, the output will be `b`.
///
/// # Notes
/// This is equivalent to the ternary operator: `condition ? a : b`. Use for concise conditional selection.
/// # Select Bool
/// Selects between two boolean values based on a condition.
/// Returns a if condition is true, b if false.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn select_bool(condition: bool, a: bool, b: bool) -> bool {
    if condition {
        a
    } else {
        b
    }
}

/// Selects between two numbers based on a boolean condition (ternary operator).
///
/// This node returns one of two input numbers depending on the value of the condition input.
/// If the condition is true, the first number is returned; if false, the second number is returned.
/// Useful for conditional logic, branching, and compact value selection.
///
/// # Inputs
/// - `condition`: The boolean condition to evaluate
/// - `a`: The number to return if the condition is true (f64)
/// - `b`: The number to return if the condition is false (f64)
///
/// # Returns
/// The selected number based on the condition
///
/// # Example
/// If `condition` is true, `a` is 10.0, and `b` is 20.0, the output will be 10.0.
/// If `condition` is false, the output will be 20.0.
///
/// # Notes
/// This is equivalent to the ternary operator: condition ? a : b.
/// # Select Number
/// Selects between two numbers based on a boolean condition.
/// Returns a if condition is true, b if false.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn select_number(condition: bool, a: f64, b: f64) -> f64 {
    if condition {
        a
    } else {
        b
    }
}

/// Selects between two strings based on a boolean condition (ternary operator).
///
/// This node returns one of two input strings depending on the value of the boolean condition.
/// Useful for conditional formatting, branching logic, or dynamic string selection in your graph.
///
/// # Inputs
/// - `condition`: The boolean condition to evaluate
/// - `a`: The string to return if the condition is true
/// - `b`: The string to return if the condition is false
///
/// # Returns
/// The selected string, either `a` or `b`
///
/// # Example
/// If `condition` is true, the output will be the value of `a`.
/// If `condition` is false, the output will be the value of `b`.
///
/// # Notes
/// This node implements a ternary operator for strings. Use it for simple conditional string selection.
/// # Select String
/// Selects between two strings based on a boolean condition.
/// Returns a if condition is true, b if false.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn select_string(condition: bool, a: String, b: String) -> String {
    if condition {
        a
    } else {
        b
    }
}

/// XOR (exclusive OR) operation.
///
/// # Inputs
/// - `a`: First boolean value
/// - `b`: Second boolean value
///
/// # Returns
/// True if exactly one input is true (exclusive OR)
///
/// # Logic XOR
/// Returns true if exactly one of the inputs is true.
#[blueprint(type: NodeTypes::pure, category: "Logic", color: "#E2A04A")]
pub fn xor(a: bool, b: bool) -> bool {
    a ^ b
}
