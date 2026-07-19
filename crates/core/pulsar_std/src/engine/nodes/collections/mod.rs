//! # Collections Module
//!
//! HashMap and HashSet operations for the Pulsar visual programming system.
//!
//! This module provides collection data structure nodes including:
//! - HashMap operations (new, insert, get, remove, contains_key, keys, values, len, is_empty, clear, merge, filter, to_json)
//! - HashSet operations (new, insert, contains, union, intersection)
//!
//! All operations work with Rust's standard library collection types.

use crate::blueprint;
use std::collections::{HashMap, HashSet};

// =============================================================================
// HashMap Operations
// =============================================================================

/// Creates a new empty HashMap.
///
/// # Returns
/// An empty HashMap<String, String>
///
/// # Example
/// The output is an empty HashMap: {}
///
/// # Notes
/// Use this node to start building a key-value collection dynamically in your graph.
/// # HashMap New
/// Creates a new empty HashMap with String keys and String values.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_new() -> HashMap<String, String> {
    HashMap::new()
}

/// Inserts a key-value pair into a HashMap.
///
/// # Inputs
/// - `map`: The input HashMap to insert into
/// - `key`: The key to insert or update
/// - `value`: The value to associate with the key
///
/// # Returns
/// The updated HashMap with the new or updated key-value pair
///
/// # Example
/// If `map` is {"a": "1"}, `key` is "b", and `value` is "2", the output will be {"a": "1", "b": "2"}.
/// If `key` is "a", the value for "a" will be updated.
///
/// # Notes
/// Use this node to add or update entries in a HashMap for fast key-based lookup and storage.
/// # HashMap Insert
/// Adds or updates a key-value pair in a HashMap.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_insert(
    map: HashMap<String, String>,
    key: String,
    value: String,
) -> HashMap<String, String> {
    let mut m = map;
    m.insert(key, value);
    m
}

/// Retrieves a value from a HashMap by key.
///
/// # Inputs
/// - `map`: The HashMap to search
/// - `key`: The key to look up
///
/// # Returns
/// The value associated with the key, or None if the key is not present
///
/// # Example
/// If `map` is {"foo": "bar"} and `key` is "foo", the output will be Some("bar").
/// If the key does not exist, the output will be None.
///
/// # Notes
/// Returns a cloned value to avoid borrowing issues. Use this node for safe, read-only access to HashMap entries.
/// # HashMap Get
/// Retrieves a value from a HashMap by key.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_get(map: HashMap<String, String>, key: String) -> Option<String> {
    map.get(&key).cloned()
}

/// Removes a key-value pair from a HashMap.
///
/// # Inputs
/// - `map`: The input HashMap to remove from
/// - `key`: The key to remove
///
/// # Returns
/// A tuple containing the updated HashMap and the removed value (if any)
///
/// # Example
/// If `map` is {"a": "1", "b": "2"} and `key` is "a", the output will be ({"b": "2"}, Some("1")).
/// If the key does not exist, the output will be (original map, None).
///
/// # Notes
/// The original HashMap is consumed and a new one is returned with the entry removed if present.
/// # HashMap Remove
/// Removes a key-value pair from a HashMap.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_remove(
    map: HashMap<String, String>,
    key: String,
) -> (HashMap<String, String>, Option<String>) {
    let mut m = map;
    let removed = m.remove(&key);
    (m, removed)
}

/// Checks if a HashMap contains a specific key.
///
/// # Inputs
/// - `map`: The HashMap to search
/// - `key`: The key to look for
///
/// # Returns
/// True if the key exists in the HashMap, false otherwise
///
/// # Example
/// If `map` is {"foo": "bar"} and `key` is "foo", the output will be true.
/// If `key` is "baz", the output will be false.
///
/// # Notes
/// Useful for checking existence before attempting to get or remove a value.
/// # HashMap Contains Key
/// Checks if a HashMap contains a specific key.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_contains_key(map: HashMap<String, String>, key: String) -> bool {
    map.contains_key(&key)
}

/// Returns all keys from a HashMap.
///
/// # Inputs
/// - `map`: The input HashMap to extract keys from
///
/// # Returns
/// A vector of strings containing all the keys in the HashMap
///
/// # Example
/// If `map` is {"a": "1", "b": "2"}, the output will be ["a", "b"].
///
/// # Notes
/// The order of keys in the output vector is not guaranteed.
/// # HashMap Keys
/// Extracts all keys from a HashMap.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_keys(map: HashMap<String, String>) -> Vec<String> {
    map.keys().cloned().collect()
}

/// Returns all values from a HashMap.
///
/// # Inputs
/// - `map`: The input HashMap to extract values from
///
/// # Returns
/// A vector of strings containing all the values in the HashMap
///
/// # Example
/// If `map` is {"a": "1", "b": "2"}, the output will be ["1", "2"].
///
/// # Notes
/// The order of values in the output vector is not guaranteed.
/// # HashMap Values
/// Extracts all values from a HashMap.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_values(map: HashMap<String, String>) -> Vec<String> {
    map.values().cloned().collect()
}

/// Returns the number of entries in a HashMap.
///
/// # Inputs
/// - `map`: The input HashMap to measure
///
/// # Returns
/// The number of entries (integer) in the HashMap
///
/// # Example
/// If `map` is {"a": "1", "b": "2"}, the output will be 2.
///
/// # Notes
/// Returns 0 for an empty HashMap.
/// # HashMap Length
/// Returns the number of key-value pairs in a HashMap.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_len(map: HashMap<String, String>) -> i64 {
    map.len() as i64
}

/// Checks if a HashMap is empty.
///
/// # Inputs
/// - `map`: The input HashMap to check
///
/// # Returns
/// True if the HashMap is empty, false otherwise
///
/// # Example
/// If `map` is {}, the output will be true. If it contains any entries, the output will be false.
///
/// # Notes
/// Useful for validating state, controlling flow, or checking if a collection needs to be populated.
/// # HashMap Is Empty
/// Checks if a HashMap contains no key-value pairs.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_is_empty(map: HashMap<String, String>) -> bool {
    map.is_empty()
}

/// Clears all entries from a HashMap.
///
/// # Inputs
/// - `map`: The input HashMap to clear
///
/// # Returns
/// An empty HashMap of the same type
///
/// # Example
/// If `map` contains {"a": "1", "b": "2"}, the output will be an empty HashMap.
///
/// # Notes
/// The original HashMap is cleared in-place and returned.
/// # HashMap Clear
/// Removes all entries from a HashMap.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_clear(map: HashMap<String, String>) -> HashMap<String, String> {
    let mut m = map;
    m.clear();
    m
}

/// Merges two HashMaps.
///
/// # Inputs
/// - `map1`: The first input HashMap
/// - `map2`: The second input HashMap (values from this map take precedence on key conflicts)
///
/// # Returns
/// A new HashMap containing all entries from both input maps
///
/// # Example
/// If `map1` is {"a": "1", "b": "2"} and `map2` is {"b": "3", "c": "4"}, the output will be {"a": "1", "b": "3", "c": "4"}.
///
/// # Notes
/// The original input maps are not modified. The result is a new HashMap containing merged entries.
/// On key conflicts, the value from the second map is used.
/// # HashMap Merge
/// Combines two HashMaps, with the second map's values taking precedence on conflicts.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_merge(
    map1: HashMap<String, String>,
    map2: HashMap<String, String>,
) -> HashMap<String, String> {
    let mut result = map1;
    for (key, value) in map2 {
        result.insert(key, value);
    }
    result
}

/// Filters HashMap entries by value pattern.
///
/// # Inputs
/// - `map`: The input HashMap to filter
/// - `pattern`: The pattern to search for in values
///
/// # Returns
/// A new HashMap containing only entries whose values contain the pattern
///
/// # Example
/// If `map` is {"a": "apple", "b": "banana", "c": "carrot"} and `pattern` is "an",
/// the output will be {"b": "banana"}.
///
/// # Notes
/// Filtering is case-sensitive. Only values containing the exact pattern are included.
/// # HashMap Filter
/// Filters HashMap entries by checking if values contain a pattern.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_filter(map: HashMap<String, String>, pattern: String) -> HashMap<String, String> {
    map.into_iter()
        .filter(|(_, value)| value.contains(&pattern))
        .collect()
}

/// Converts a HashMap to a JSON string representation.
///
/// # Inputs
/// - `map`: The input HashMap to convert to JSON
///
/// # Returns
/// A JSON string representing the HashMap
///
/// # Example
/// If `map` is {"a": "1", "b": "2"}, the output will be "{\"a\":\"1\",\"b\":\"2\"}".
///
/// # Notes
/// This implementation escapes double quotes in keys and values. The output is a flat JSON object with string keys and values.
/// For more complex or nested data, consider using a dedicated JSON serialization library.
/// # HashMap To JSON
/// Converts a HashMap to a JSON string representation.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashmap_to_json(map: HashMap<String, String>) -> String {
    let mut json = String::from("{");
    let entries: Vec<String> = map
        .iter()
        .map(|(k, v)| {
            format!(
                "\"{}\":\"{}\"",
                k.replace("\"", "\\\""),
                v.replace("\"", "\\\"")
            )
        })
        .collect();
    json.push_str(&entries.join(","));
    json.push('}');
    json
}

// =============================================================================
// HashSet Operations
// =============================================================================

/// Creates a new empty HashSet.
///
/// # Returns
/// An empty HashSet<String>
///
/// # Example
/// The output is an empty HashSet: {}
///
/// # Notes
/// Use this node to start building a set of unique values dynamically in your graph.
/// # HashSet New
/// Creates a new empty HashSet of Strings.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashset_new() -> HashSet<String> {
    HashSet::new()
}

/// Inserts a value into a HashSet.
///
/// # Inputs
/// - `set`: The input HashSet to insert into
/// - `value`: The value to insert
///
/// # Returns
/// The updated HashSet containing the value (if not already present)
///
/// # Example
/// If `set` is {"apple"} and `value` is "banana", the output will be {"apple", "banana"}.
/// If `value` is "apple", the output will remain {"apple"}.
///
/// # Notes
/// The original HashSet is consumed and a new one is returned with the value inserted if not already present.
/// # HashSet Insert
/// Adds a value to a HashSet.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashset_insert(set: HashSet<String>, value: String) -> HashSet<String> {
    let mut s = set;
    s.insert(value);
    s
}

/// Checks if a HashSet contains a value.
///
/// # Inputs
/// - `set`: The HashSet to search
/// - `value`: The value to look for
///
/// # Returns
/// True if the value exists in the HashSet, false otherwise
///
/// # Example
/// If `set` is {"apple", "banana"} and `value` is "banana", the output will be true.
/// If `value` is "carrot", the output will be false.
///
/// # Notes
/// Useful for membership checks, filtering, and set operations.
/// # HashSet Contains
/// Checks if a HashSet contains a specific value.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashset_contains(set: HashSet<String>, value: String) -> bool {
    set.contains(&value)
}

/// Computes the union of two HashSets.
///
/// # Inputs
/// - `set1`: The first input HashSet
/// - `set2`: The second input HashSet
///
/// # Returns
/// A new HashSet containing all unique elements from both sets
///
/// # Example
/// If `set1` is {"apple", "banana"} and `set2` is {"banana", "carrot"}, the output will be {"apple", "banana", "carrot"}.
///
/// # Notes
/// The original input sets are not modified. The result is a new HashSet containing the union.
/// # HashSet Union
/// Computes the union of two HashSets.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashset_union(set1: HashSet<String>, set2: HashSet<String>) -> HashSet<String> {
    set1.union(&set2).cloned().collect()
}

/// Computes the intersection of two HashSets.
///
/// # Inputs
/// - `set1`: The first input HashSet
/// - `set2`: The second input HashSet
///
/// # Returns
/// A new HashSet containing elements that exist in both input sets
///
/// # Example
/// If `set1` is {"apple", "banana"} and `set2` is {"banana", "carrot"}, the output will be {"banana"}.
///
/// # Notes
/// The original input sets are not modified. The result is a new HashSet containing the intersection.
/// # HashSet Intersection
/// Computes the intersection of two HashSets.
#[blueprint(type: NodeTypes::pure, category: "Collections", color: "#9B59B6")]
pub fn hashset_intersection(set1: HashSet<String>, set2: HashSet<String>) -> HashSet<String> {
    set1.intersection(&set2).cloned().collect()
}
