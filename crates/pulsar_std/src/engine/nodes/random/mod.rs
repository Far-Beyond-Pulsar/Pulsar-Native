//! # Random Module
//!
//! Random number generation nodes for the Pulsar visual programming system.
//!
//! This module provides nodes for:
//! - Random integer generation
//! - Random float generation
//! - Random boolean generation
//! - Random selection from collections
//! - Random distributions

use crate::blueprint;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};

/// Generate a pseudo-random integer.
///
/// # Returns
/// A pseudo-random integer
///
/// # Random Int
/// Generates a pseudo-random integer.
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_int() -> i64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    hasher.finish() as i64
}

/// Generate a pseudo-random integer in a range.
///
/// # Inputs
/// - `min`: Minimum value (inclusive)
/// - `max`: Maximum value (exclusive)
///
/// # Returns
/// A pseudo-random integer in the range [min, max)
///
/// # Random Int Range
/// Generates a pseudo-random integer in a specified range.
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_int_range(min: i64, max: i64) -> i64 {
    if max <= min {
        return min;
    }
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    let range = (max - min) as u64;
    min + ((hasher.finish() % range) as i64)
}

/// Generate a pseudo-random float between 0.0 and 1.0.
///
/// # Returns
/// A pseudo-random float in the range [0.0, 1.0)
///
/// # Random Float
/// Generates a pseudo-random float between 0.0 and 1.0.
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_float() -> f64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    (hasher.finish() as f64) / (u64::MAX as f64)
}

/// Generate a pseudo-random float in a range.
///
/// # Inputs
/// - `min`: Minimum value (inclusive)
/// - `max`: Maximum value (inclusive)
///
/// # Returns
/// A pseudo-random float in the range [min, max]
///
/// # Random Float Range
/// Generates a pseudo-random float in a specified range.
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_float_range(min: f64, max: f64) -> f64 {
    if max <= min {
        return min;
    }
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    let t = (hasher.finish() as f64) / (u64::MAX as f64);
    min + (max - min) * t
}

/// Generate a pseudo-random boolean.
///
/// # Returns
/// A pseudo-random boolean (true or false)
///
/// # Random Bool
/// Generates a pseudo-random boolean value.
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_bool() -> bool {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    (hasher.finish() & 1) == 1
}

/// Generate a pseudo-random boolean with custom probability.
///
/// # Inputs
/// - `probability`: Probability of returning true (0.0 to 1.0)
///
/// # Returns
/// A pseudo-random boolean based on the probability
///
/// # Random Bool Probability
/// Generates a pseudo-random boolean with a specified probability of being true.
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_bool_probability(probability: f64) -> bool {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    let t = (hasher.finish() as f64) / (u64::MAX as f64);
    t < probability.clamp(0.0, 1.0)
}

/// Generate a random angle in degrees (0-360).
///
/// # Returns
/// A random angle in degrees
///
/// # Random Angle Degrees
/// Generates a random angle in degrees (0-360).
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_angle_degrees() -> f64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    ((hasher.finish() as f64) / (u64::MAX as f64)) * 360.0
}

/// Generate a random angle in radians (0-2π).
///
/// # Returns
/// A random angle in radians
///
/// # Random Angle Radians
/// Generates a random angle in radians (0-2π).
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_angle_radians() -> f64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    ((hasher.finish() as f64) / (u64::MAX as f64)) * std::f64::consts::TAU
}

/// Generate a random sign (-1 or 1).
///
/// # Returns
/// Either -1 or 1 with equal probability
///
/// # Random Sign
/// Generates a random sign (-1 or 1).
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_sign() -> i64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    if (hasher.finish() & 1) == 1 {
        1
    } else {
        -1
    }
}

/// Generate a random value from a normal distribution (simplified).
///
/// # Inputs
/// - `mean`: The mean of the distribution
/// - `std_dev`: The standard deviation
///
/// # Returns
/// A random value from the normal distribution
///
/// # Random Normal
/// Generates a random value from a normal distribution (Box-Muller approximation).
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_normal(mean: f64, std_dev: f64) -> f64 {
    // Simplified Box-Muller transform
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    let u1 = (hasher.finish() as f64) / (u64::MAX as f64);

    std::time::SystemTime::now().hash(&mut hasher);
    let u2 = (hasher.finish() as f64) / (u64::MAX as f64);

    let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
    mean + std_dev * z
}

/// Randomly shuffle/permute a number (for use as seed).
///
/// # Inputs
/// - `value`: The value to shuffle
///
/// # Returns
/// A shuffled/permuted version of the input
///
/// # Random Shuffle Seed
/// Shuffles a number to create a pseudo-random seed.
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn shuffle_seed(value: i64) -> i64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    value.hash(&mut hasher);
    hasher.finish() as i64
}

/// Generate a random color component (0-255).
///
/// # Returns
/// A random integer in the range [0, 255]
///
/// # Random Color Component
/// Generates a random color component value (0-255).
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_color_component() -> i64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    (hasher.finish() % 256) as i64
}

/// Generate a random percentage (0-100).
///
/// # Returns
/// A random integer in the range [0, 100]
///
/// # Random Percentage
/// Generates a random percentage value (0-100).
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_percentage() -> i64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    (hasher.finish() % 101) as i64
}

/// Generate a random dice roll (1-6).
///
/// # Returns
/// A random integer in the range [1, 6]
///
/// # Random Dice Roll
/// Simulates a six-sided dice roll (1-6).
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_dice() -> i64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    ((hasher.finish() % 6) + 1) as i64
}

/// Generate a random dice roll with custom sides.
///
/// # Inputs
/// - `sides`: Number of sides on the dice
///
/// # Returns
/// A random integer in the range [1, sides]
///
/// # Random Dice Custom
/// Simulates a dice roll with a custom number of sides.
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn random_dice_custom(sides: i64) -> i64 {
    if sides <= 0 {
        return 1;
    }
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    ((hasher.finish() % (sides as u64)) + 1) as i64
}

/// Flip a coin (heads or tails).
///
/// # Returns
/// True for heads, false for tails
///
/// # Random Coin Flip
/// Simulates a coin flip (50/50 chance).
#[blueprint(type: crate::NodeTypes::pure, category: "Random", color: "#E74C3C")]
pub fn coin_flip() -> bool {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    (hasher.finish() & 1) == 1
}
