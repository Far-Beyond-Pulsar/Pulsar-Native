//! # Array Module
//!
//! Array and vector operations for the Pulsar visual programming system.
//!
//! This module provides array/vector manipulation nodes including:
//! - Creation and initialization (new)
//! - Element access (get, first, last)
//! - Modification (push, pop, set, clear)
//! - Information (length, is_empty, contains)
//! - Transformations (reverse, sort, slice, concat)
//!
//! All operations work with Rust's `Vec<T>` type for dynamic arrays.

use crate::blueprint;

// =============================================================================
// Array Creation
// =============================================================================

/// Creates a new empty vector.
///
/// # Returns
/// A new empty vector
///
/// # Example
/// The output is an empty vector: []
///
/// # Notes
/// Use this node to initialize a vector before adding elements.
/// The type of the vector will be inferred from usage.
/// # Array New
/// Creates a new empty vector.
#[blueprint(type: NodeTypes::pure, category: "Array", color: "#50E3C2")]
pub fn array_new<T>() -> Vec<T> {
    Vec::new()
}

// =============================================================================
// Element Modification
// =============================================================================

/// Pushes an element to the end of a vector.
///
/// # Inputs
/// - `array`: The input vector
/// - `item`: The item to push
///
/// # Returns
/// The updated vector with the new item appended
///
/// # Example
/// If `array` is [1, 2] and `item` is 3, the output will be [1, 2, 3].
///
/// # Notes
/// This operation adds the element to the end of the vector.
/// # Array Push
/// Appends an element to the end of a vector.
#[blueprint(type: NodeTypes::fn_, category: "Array", color: "#50E3C2")]
pub fn array_push<T>(array: Vec<T>, item: T) -> Vec<T> {
    let mut arr = array;
    arr.push(item);
    arr
}

/// Pops the last element from a vector.
///
/// # Inputs
/// - `array`: The input vector
///
/// # Returns
/// A tuple containing the updated vector and the popped element (or None if empty)
///
/// # Example
/// If the input vector is [1, 2, 3], the output will be ([1, 2], Some(3)).
/// If the vector is empty, the output will be ([], None).
///
/// # Notes
/// This operation removes and returns the last element.
/// # Array Pop
/// Removes and returns the last element from a vector.
#[blueprint(type: NodeTypes::fn_, category: "Array", color: "#50E3C2")]
pub fn array_pop<T: Clone>(array: Vec<T>) -> (Vec<T>, Option<T>) {
    let mut arr = array;
    let popped = arr.pop();
    (arr, popped)
}

/// Sets an element at a specific index in a vector.
///
/// # Inputs
/// - `array`: The input vector
/// - `index`: The index to set (usize)
/// - `value`: The value to set at the index
///
/// # Returns
/// The updated vector with the new value at the specified index
///
/// # Example
/// If `array` is [1, 2, 3], `index` is 1, and `value` is 99,
/// the output will be [1, 99, 3].
///
/// # Notes
/// If the index is out of bounds, no change is made and the original vector is returned.
/// # Array Set
/// Sets the value at a specific index in a vector.
#[blueprint(type: NodeTypes::fn_, category: "Array", color: "#50E3C2")]
pub fn array_set<T>(array: Vec<T>, index: i64, value: T) -> Vec<T> {
    let mut arr = array;
    let idx = index as usize;
    if idx < arr.len() {
        arr[idx] = value;
    }
    arr
}

/// Clears all elements from a vector.
///
/// # Inputs
/// - `array`: The input vector to be cleared
///
/// # Returns
/// The cleared (empty) vector
///
/// # Example
/// If `array` is [1, 2, 3], the output will be [].
///
/// # Notes
/// This operation removes all elements, resulting in an empty vector.
/// # Array Clear
/// Removes all elements from a vector.
#[blueprint(type: NodeTypes::fn_, category: "Array", color: "#50E3C2")]
pub fn array_clear<T>(array: Vec<T>) -> Vec<T> {
    let mut arr = array;
    arr.clear();
    arr
}

// =============================================================================
// Element Access
// =============================================================================

/// Gets an element at a specific index.
///
/// # Inputs
/// - `array`: The input vector
/// - `index`: The index to get (0-based)
///
/// # Returns
/// The element at the index, or None if out of bounds
///
/// # Example
/// If `array` is [10, 20, 30] and `index` is 1, the output will be Some(20).
/// If `index` is 5, the output will be None.
///
/// # Notes
/// Indexing is zero-based. If the index is negative or greater than or equal to the vector length, None is returned.
/// # Array Get
/// Retrieves the element at a specific index.
#[blueprint(type: NodeTypes::pure, category: "Array", color: "#50E3C2")]
pub fn array_get<T: Clone>(array: Vec<T>, index: i64) -> Option<T> {
    let idx = index as usize;
    array.get(idx).cloned()
}

/// Returns the first element of a vector.
///
/// # Inputs
/// - `array`: The input vector
///
/// # Returns
/// The first element of the vector, or None if the vector is empty
///
/// # Example
/// If the input vector is [10, 20, 30], the output will be Some(10).
/// If the input vector is empty, the output will be None.
///
/// # Notes
/// This is equivalent to getting the element at index 0.
/// # Array First
/// Returns the first element of a vector.
#[blueprint(type: NodeTypes::pure, category: "Array", color: "#50E3C2")]
pub fn array_first<T: Clone>(array: Vec<T>) -> Option<T> {
    array.first().cloned()
}

/// Returns the last element of a vector.
///
/// # Inputs
/// - `array`: The input vector
///
/// # Returns
/// The last element, or None if the vector is empty
///
/// # Example
/// If `array` is [1, 2, 3], the output will be Some(3).
/// If the vector is empty, the output will be None.
///
/// # Notes
/// This is equivalent to getting the element at index (length - 1).
/// # Array Last
/// Returns the last element of a vector.
#[blueprint(type: NodeTypes::pure, category: "Array", color: "#50E3C2")]
pub fn array_last<T: Clone>(array: Vec<T>) -> Option<T> {
    array.last().cloned()
}

// =============================================================================
// Array Information
// =============================================================================

/// Returns the length of a vector.
///
/// # Inputs
/// - `array`: The input vector
///
/// # Returns
/// The length of the vector (number of elements) as an i64
///
/// # Example
/// If `array` is [1, 2, 3], the output will be 3.
/// If the vector is empty, the output will be 0.
///
/// # Notes
/// The length represents the total number of elements in the vector.
/// # Array Length
/// Returns the number of elements in a vector.
#[blueprint(type: NodeTypes::pure, category: "Array", color: "#50E3C2")]
pub fn array_length<T>(array: Vec<T>) -> i64 {
    array.len() as i64
}

/// Checks if a vector is empty.
///
/// # Inputs
/// - `array`: The input vector
///
/// # Returns
/// True if the vector is empty, false otherwise
///
/// # Example
/// If `array` is [], the output will be true.
/// If it contains elements like [1, 2], the output will be false.
///
/// # Notes
/// This is equivalent to checking if the length is 0.
/// # Array Is Empty
/// Checks if a vector contains no elements.
#[blueprint(type: NodeTypes::pure, category: "Array", color: "#50E3C2")]
pub fn array_is_empty<T>(array: Vec<T>) -> bool {
    array.is_empty()
}

/// Checks if a vector contains a specific element.
///
/// # Inputs
/// - `array`: The input vector
/// - `item`: The item to check for existence
///
/// # Returns
/// Returns `true` if the vector contains the item, `false` otherwise
///
/// # Example
/// If `array` is [1, 2, 3] and `item` is 2, the output will be true.
/// If `item` is 5, the output will be false.
///
/// # Notes
/// This performs a linear search through the vector.
/// The element type must implement PartialEq for comparison.
/// # Array Contains
/// Checks if a vector contains a specific element.
#[blueprint(type: NodeTypes::pure, category: "Array", color: "#50E3C2")]
pub fn array_contains<T: PartialEq>(array: Vec<T>, item: T) -> bool {
    array.contains(&item)
}

// =============================================================================
// Array Transformations
// =============================================================================

/// Extracts a slice from a vector.
///
/// # Inputs
/// - `array`: The input vector
/// - `start`: The start index (inclusive)
/// - `end`: The end index (exclusive)
///
/// # Returns
/// The sliced vector, or empty if indices are invalid
///
/// # Example
/// If `array` is [1, 2, 3, 4, 5], `start` is 1, and `end` is 4,
/// the output will be [2, 3, 4].
///
/// # Notes
/// Indices must satisfy 0 <= start <= end <= array length.
/// If indices are out of bounds or invalid, an empty vector is returned.
/// # Array Slice
/// Extracts a subvector from start (inclusive) to end (exclusive).
#[blueprint(type: NodeTypes::pure, category: "Array", color: "#50E3C2")]
pub fn array_slice<T: Clone>(array: Vec<T>, start: i64, end: i64) -> Vec<T> {
    let start_idx = start as usize;
    let end_idx = end as usize;
    if start_idx < array.len() && end_idx <= array.len() && start_idx <= end_idx {
        array[start_idx..end_idx].to_vec()
    } else {
        Vec::new()
    }
}

/// Reverses the order of elements in a vector.
///
/// # Inputs
/// - `array`: The input vector
///
/// # Returns
/// The reversed vector
///
/// # Example
/// If the input vector is [1, 2, 3], the output will be [3, 2, 1].
///
/// # Notes
/// This creates a new vector with elements in reverse order.
/// The original order is not preserved.
/// # Array Reverse
/// Reverses the order of elements in a vector.
#[blueprint(type: NodeTypes::fn_, category: "Array", color: "#50E3C2")]
pub fn array_reverse<T>(array: Vec<T>) -> Vec<T> {
    let mut arr = array;
    arr.reverse();
    arr
}

/// Sorts a vector in ascending order.
///
/// # Inputs
/// - `array`: The input vector to be sorted
///
/// # Returns
/// The sorted vector
///
/// # Example
/// If `array` is [3, 1, 2], the output will be [1, 2, 3].
///
/// # Notes
/// Sorting is based on the natural ordering of the element type.
/// For custom types, ensure they implement `Ord`.
/// The original input vector is not modified; a sorted copy is returned.
/// # Array Sort
/// Sorts a vector in ascending order.
#[blueprint(type: NodeTypes::fn_, category: "Array", color: "#50E3C2")]
pub fn array_sort<T: Ord>(array: Vec<T>) -> Vec<T> {
    let mut arr = array;
    arr.sort();
    arr
}

/// Concatenates two vectors.
///
/// # Inputs
/// - `a`: The first input vector
/// - `b`: The second input vector
///
/// # Returns
/// A new vector containing all elements of `a` followed by all elements of `b`
///
/// # Example
/// If `a` is [1, 2] and `b` is [3, 4], the output will be [1, 2, 3, 4].
///
/// # Notes
/// This creates a new vector with all elements from both input vectors.
/// The first vector's elements appear first, followed by the second vector's elements.
/// # Array Concat
/// Concatenates two vectors into a single vector.
#[blueprint(type: NodeTypes::fn_, category: "Array", color: "#50E3C2")]
pub fn array_concat<T>(a: Vec<T>, b: Vec<T>) -> Vec<T> {
    let mut result = a;
    result.extend(b);
    result
}
