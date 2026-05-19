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

/// Assert that two f32 values are approximately equal within an epsilon.
/// Panics if |actual - expected| >= epsilon.
///
/// # Inputs
/// - `actual`: The computed f32 value
/// - `expected`: The expected f32 value
/// - `epsilon`: Maximum allowed difference (e.g. 0.0001)
///
/// # Assert Equal F32
/// Fails the blueprint if the f32 values differ by more than epsilon.
#[blueprint(type: NodeTypes::fn_, category: "Testing", color: "#E74C3C")]
pub fn assert_eq_f32(actual: f32, expected: f32, epsilon: f32) {
    if (actual - expected).abs() >= epsilon {
        panic!(
            "Blueprint assert_eq_f32 failed: actual={} expected={} diff={} epsilon={}",
            actual,
            expected,
            (actual - expected).abs(),
            epsilon
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

// =============================================================================
// Complex-type stress nodes
//
// These nodes use concrete, non-generic types that are large, non-trivially
// laid out, and heap-allocated — the hardest cases for the arena dispatcher.
// =============================================================================

/// Produces a concrete `Vec<(i128, i64, bool)>` with three known elements:
///   [0] = (i128::MAX,   -1,   true )
///   [1] = (0,            0,   false)
///   [2] = (42,         999,   true )
///
/// # Make Mixed Vec
/// Creates a Vec of (i128, i64, bool) tuples for stress-testing the dispatcher.
#[blueprint(type: NodeTypes::pure, category: "Testing", color: "#E74C3C")]
pub fn make_mixed_vec() -> Vec<(i128, i64, bool)> {
    vec![
        (i128::MAX, -1i64, true),
        (0i128, 0i64, false),
        (42i128, 999i64, true),
    ]
}

/// Consumes a `Vec<(i128, i64, bool)>` and returns true iff it contains exactly
/// the three elements produced by `make_mixed_vec`.
///
/// Panics-on-assertion-failure path lets the test runner detect bugs without
/// any special error-propagation plumbing.
///
/// # Mixed Vec Check
/// Verifies the contents of a Vec<(i128, i64, bool)> against known values.
#[blueprint(type: NodeTypes::pure, category: "Testing", color: "#E74C3C")]
pub fn mixed_vec_check(v: Vec<(i128, i64, bool)>) -> bool {
    v.len() == 3
        && v[0] == (i128::MAX, -1i64, true)
        && v[1] == (0i128, 0i64, false)
        && v[2] == (42i128, 999i64, true)
}

/// Consumes a `Vec<(i128, i64, bool)>` and returns its length as i64.
///
/// # Mixed Vec Len
/// Returns the length of a Vec<(i128, i64, bool)>.
#[blueprint(type: NodeTypes::pure, category: "Testing", color: "#E74C3C")]
pub fn mixed_vec_len(v: Vec<(i128, i64, bool)>) -> i64 {
    v.len() as i64
}

/// Consumes a `Vec<(i128, i64, bool)>` and sums: each i128 as i64 (wrapping)
/// + each i64 + each bool (1 if true).  Returns the combined sum as i64.
/// Tests that the dispatcher correctly threads an 8-element struct through the
/// arena byte-by-byte.
///
/// # Mixed Vec Sum
/// Computes a checksum of all fields in a Vec<(i128, i64, bool)>.
#[blueprint(type: NodeTypes::pure, category: "Testing", color: "#E74C3C")]
pub fn mixed_vec_sum(v: Vec<(i128, i64, bool)>) -> i64 {
    v.iter().fold(0i64, |acc, &(a, b, c)| {
        acc.wrapping_add(a as i64)
            .wrapping_add(b)
            .wrapping_add(c as i64)
    })
}
