use crate::{blueprint, NodeTypes};

/// Assert that a boolean condition is true.
/// Panics with a descriptive message if the condition is false.
///
/// # Inputs
/// - `condition`: The boolean value to assert
///
/// # Assert True
/// Fails the blueprint if condition is false. Use in test graphs to verify correctness.
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_true(condition: bool) {
    if !condition {
        panic!("Blueprint assert_true failed: condition was false");
    }
}

/// Assert that a boolean condition is false.
///
/// # Assert False
/// Fails the blueprint if condition is true.
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_false(condition: bool) {
    if condition {
        panic!("Blueprint assert_false failed: condition was true");
    }
}

/// Assert that two integers are equal.
/// Panics if actual != expected.
///
/// # Inputs
/// - `actual`: The computed value
/// - `expected`: The expected value
///
/// # Assert Equal Int
/// Fails the blueprint if the two integer values are not equal.
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_eq_int(actual: i64, expected: i64) {
    if actual != expected {
        panic!(
            "Blueprint assert_eq_int failed: actual={} expected={}",
            actual, expected
        );
    }
}

/// Assert that two integers are NOT equal.
///
/// # Assert Not Equal Int
/// Fails the blueprint if the two integer values are equal.
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_ne_int(actual: i64, expected: i64) {
    if actual == expected {
        panic!(
            "Blueprint assert_ne_int failed: both values were {}",
            actual
        );
    }
}

/// Assert that two floats are approximately equal within an epsilon.
/// Panics if |actual - expected| >= epsilon.
///
/// # Inputs
/// - `actual`: The computed float value
/// - `expected`: The expected float value
/// - `epsilon`: Maximum allowed difference (e.g. 0.0001)
///
/// # Assert Equal Float
/// Fails the blueprint if the values differ by more than epsilon.
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_eq_float(actual: f64, expected: f64, epsilon: f64) {
    if (actual - expected).abs() >= epsilon {
        panic!(
            "Blueprint assert_eq_float failed: actual={} expected={} diff={} epsilon={}",
            actual,
            expected,
            (actual - expected).abs(),
            epsilon
        );
    }
}

/// Assert that an integer is greater than a threshold.
///
/// # Assert Greater Than Int
/// Fails the blueprint if actual <= threshold.
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_gt_int(actual: i64, threshold: i64) {
    if actual <= threshold {
        panic!(
            "Blueprint assert_gt_int failed: {} is not > {}",
            actual, threshold
        );
    }
}

/// Assert that an integer is less than a threshold.
///
/// # Assert Less Than Int
/// Fails the blueprint if actual >= threshold.
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_lt_int(actual: i64, threshold: i64) {
    if actual >= threshold {
        panic!(
            "Blueprint assert_lt_int failed: {} is not < {}",
            actual, threshold
        );
    }
}

/// Assert that an integer is within [lo, hi] inclusive.
///
/// # Assert In Range Int
/// Fails the blueprint if actual is outside [lo, hi].
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_in_range_int(actual: i64, lo: i64, hi: i64) {
    if actual < lo || actual > hi {
        panic!(
            "Blueprint assert_in_range_int failed: {} not in [{}, {}]",
            actual, lo, hi
        );
    }
}
