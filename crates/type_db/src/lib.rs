//! # Type Database
//!
//! This crate provides an in-memory, thread-safe database for storing and searching user-defined runtime types.
//! It is designed for use in game engines or similar systems where types may be registered, queried, and removed at runtime.
//!
//! ## Features
//! - Fast registration and lookup by ID, name, or category
//! - Case-insensitive and fuzzy search
//! - Thread-safe with concurrent access (using DashMap)
//! - Simple API for integration
//!
//! ## Example
//! ```rust
//! use type_db::{TypeDatabase};
//! let mut db = TypeDatabase::new();
//! let id = db.register("Vector3", Some("Math".to_string()), Some("3D vector".to_string()));
//! let found = db.get(id);
//! assert!(found.is_some());
//! ```

use dashmap::DashMap;

/// Represents a runtime type created by the user in the engine.
///
/// Each `TypeInfo` contains a unique ID, a display name, an optional category for grouping,
/// and an optional description. This struct is intended to be lightweight and easily clonable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeInfo {
    /// Unique identifier for the type
    pub id: u64,
    /// Display name of the type
    pub name: String,
    /// Optional category for grouping types
    pub category: Option<String>,
    /// Optional description of the type
    pub description: Option<String>,
}


/// An in-memory, thread-safe database for storing and searching user-created runtime types.
///
/// `TypeDatabase` supports fast registration, removal, and lookup of types by ID, name, or category.
/// It uses `DashMap` internally for concurrent access, making it suitable for multi-threaded environments.
///
/// # Example
/// ```rust
/// use type_db::TypeDatabase;
/// 
/// let mut db = TypeDatabase::new();
/// let id = db.register("Vector3", Some("Math".to_string()), None);
/// let found = db.get(id);
/// assert!(found.is_some());
/// ```

#[derive(Debug, Default)]
pub struct TypeDatabase {
    /// Map of type ID to type info
    types: DashMap<u64, TypeInfo>,
    /// Index for name-based lookups (lowercase name -> type IDs)
    name_index: DashMap<String, Vec<u64>>,
    /// Index for category-based lookups
    category_index: DashMap<String, Vec<u64>>,
    /// Next available type ID
    next_id: u64,
}

impl TypeDatabase {
    /// Creates a new, empty type database.
    ///
    /// # Returns
    /// A new `TypeDatabase` instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new type and returns its assigned unique ID.
    ///
    /// # Arguments
    /// * `name` - The display name of the type (case-insensitive for lookups).
    /// * `category` - Optional category for grouping types.
    /// * `description` - Optional description for documentation/UI.
    ///
    /// # Returns
    /// The unique ID assigned to the new type.
    ///
    /// # Example
    /// ```rust
    /// use type_db::TypeDatabase;
    /// 
    /// let mut db = TypeDatabase::new();
    /// let id = db.register("Vector3", Some("Math".to_string()), None);
    /// ```
    pub fn register(&mut self, name: impl Into<String>, category: Option<String>, description: Option<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let name = name.into();
        let type_info = TypeInfo {
            id,
            name: name.clone(),
            category: category.clone(),
            description,
        };

        // Add to name index
        self.name_index
            .entry(name.to_lowercase())
            .or_insert_with(Vec::new)
            .push(id);

        // Add to category index
        if let Some(cat) = &category {
            self.category_index
                .entry(cat.to_lowercase())
                .or_insert_with(Vec::new)
                .push(id);
        }

        self.types.insert(id, type_info);
        id
    }

    /// Removes a type by its ID.
    ///
    /// # Arguments
    /// * `id` - The unique ID of the type to remove.
    ///
    /// # Returns
    /// The removed `TypeInfo` if it existed, or `None` if not found.
    ///
    /// # Example
    /// ```rust
    /// use type_db::TypeDatabase;
    ///
    /// let mut db = TypeDatabase::new();
    /// let id = db.register("Foo", None, None);
    /// let removed = db.unregister(id);
    /// assert!(removed.is_some());
    /// ```
    pub fn unregister(&mut self, id: u64) -> Option<TypeInfo> {
        if let Some((_, type_info)) = self.types.remove(&id) {
            // Remove from name index
            if let Some(mut ids) = self.name_index.get_mut(&type_info.name.to_lowercase()) {
                ids.retain(|&i| i != id);
            }

            // Remove from category index
            if let Some(cat) = &type_info.category {
                if let Some(mut ids) = self.category_index.get_mut(&cat.to_lowercase()) {
                    ids.retain(|&i| i != id);
                }
            }

            Some(type_info)
        } else {
            None
        }
    }

    /// Gets a type by its unique ID.
    ///
    /// # Arguments
    /// * `id` - The unique ID of the type.
    ///
    /// # Returns
    /// `Some(TypeInfo)` if found, or `None` if not found.
    pub fn get(&self, id: u64) -> Option<TypeInfo> {
        self.types.get(&id).map(|v| v.clone())
    }

    /// Gets all types with the given exact name (case-insensitive).
    ///
    /// # Arguments
    /// * `name` - The name to search for (case-insensitive).
    ///
    /// # Returns
    /// A vector of all matching `TypeInfo`.
    pub fn get_by_name(&self, name: &str) -> Vec<TypeInfo> {
        self.name_index
            .get(&name.to_lowercase())
            .map(|ids| ids.iter().filter_map(|id| self.types.get(id).map(|v| v.clone())).collect())
            .unwrap_or_default()
    }

    /// Searches for types whose names contain the query string (case-insensitive substring match).
    ///
    /// # Arguments
    /// * `query` - The substring to search for (case-insensitive).
    ///
    /// # Returns
    /// A vector of all matching `TypeInfo`.
    pub fn search(&self, query: &str) -> Vec<TypeInfo> {
        let query_lower = query.to_lowercase();
        self.types
            .iter()
            .filter(|t| t.name.to_lowercase().contains(&query_lower))
            .map(|t| t.clone())
            .collect()
    }

    /// Searches for types with fuzzy matching on the name.
    ///
    /// Uses a simple scoring algorithm to rank results by relevance.
    ///
    /// # Arguments
    /// * `query` - The fuzzy pattern to search for (case-insensitive, non-contiguous).
    ///
    /// # Returns
    /// A vector of all matching `TypeInfo`, sorted by descending score.
    pub fn search_fuzzy(&self, query: &str) -> Vec<TypeInfo> {
        let query_lower = query.to_lowercase();
        let query_chars: Vec<char> = query_lower.chars().collect();

        let mut results: Vec<(TypeInfo, i32)> = self
            .types
            .iter()
            .filter_map(|t| {
                let score = fuzzy_match(&query_chars, &t.name.to_lowercase());
                if score > 0 {
                    Some((t.clone(), score))
                } else {
                    None
                }
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| b.1.cmp(&a.1));
        results.into_iter().map(|(t, _)| t).collect()
    }

    /// Gets all types in a given category (case-insensitive).
    ///
    /// # Arguments
    /// * `category` - The category name to search for (case-insensitive).
    ///
    /// # Returns
    /// A vector of all types in the category.
    pub fn get_by_category(&self, category: &str) -> Vec<TypeInfo> {
        self.category_index
            .get(&category.to_lowercase())
            .map(|ids| ids.iter().filter_map(|id| self.types.get(id).map(|v| v.clone())).collect())
            .unwrap_or_default()
    }

    /// Returns all registered types in the database.
    ///
    /// # Returns
    /// A vector of all `TypeInfo` currently registered.
    pub fn all(&self) -> Vec<TypeInfo> {
        self.types.iter().map(|t| t.clone()).collect()
    }

    /// Returns the number of registered types in the database.
    ///
    /// # Returns
    /// The number of types currently registered.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Returns `true` if no types are registered in the database.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Clears all registered types from the database.
    ///
    /// This removes all types, names, and category indices.
    pub fn clear(&mut self) {
        self.types.clear();
        self.name_index.clear();
        self.category_index.clear();
    }
}

/// Simple fuzzy matching algorithm that returns a score.
///
/// Returns a positive score if all pattern characters are found in order in the text.
/// Higher scores are given for consecutive matches and matches at word/segment boundaries.
/// Returns 0 if the pattern is not fully matched.
///
/// # Arguments
/// * `pattern` - The pattern as a slice of lowercase chars.
/// * `text` - The text to search (already lowercase).
///
/// # Returns
/// An integer score (0 = no match, higher is better).
fn fuzzy_match(pattern: &[char], text: &str) -> i32 {
    let text_chars: Vec<char> = text.chars().collect();
    let mut pattern_idx = 0;
    let mut score = 0;
    let mut prev_match = false;

    for (i, &c) in text_chars.iter().enumerate() {
        if pattern_idx < pattern.len() && c == pattern[pattern_idx] {
            pattern_idx += 1;
            score += 1;

            // Bonus for consecutive matches
            if prev_match {
                score += 2;
            }

            // Bonus for matching at start or after separator
            if i == 0 || matches!(text_chars.get(i.wrapping_sub(1)), Some('_' | ' ' | '-')) {
                score += 3;
            }

            prev_match = true;
        } else {
            prev_match = false;
        }
    }

    // Only return score if all pattern characters were matched
    if pattern_idx == pattern.len() {
        score
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // # TypeDatabase Tests
    //
    // This module contains unit and performance tests for the TypeDatabase.
    // It covers registration, lookup, removal, edge cases, and basic performance/concurrency.
    //
    // Performance tests are not strict benchmarks, but will fail if operations are unreasonably slow.

    use std::time::Instant;
    use std::thread;
    use std::sync::Arc;

    #[test]
    fn test_register_and_get() {
        let mut db = TypeDatabase::new();
        let id = db.register("Vector3", Some("Math".to_string()), None);

        let type_info = db.get(id).unwrap();
        assert_eq!(type_info.name, "Vector3");
        assert_eq!(type_info.category, Some("Math".to_string()));
    }

    #[test]
    fn test_search() {
        let mut db = TypeDatabase::new();
        db.register("Vector2", Some("Math".to_string()), None);
        db.register("Vector3", Some("Math".to_string()), None);
        db.register("String", Some("Primitives".to_string()), None);

        let results = db.search("vec");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_fuzzy_search() {
        let mut db = TypeDatabase::new();
        db.register("PlayerController", None, None);
        db.register("EnemyController", None, None);
        db.register("GameManager", None, None);

        let results = db.search_fuzzy("pc");
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "PlayerController");
    }

    #[test]
    fn test_category_lookup() {
        let mut db = TypeDatabase::new();
        db.register("Vector2", Some("Math".to_string()), None);
        db.register("Vector3", Some("Math".to_string()), None);
        db.register("String", Some("Primitives".to_string()), None);

        let math_types = db.get_by_category("math");
        assert_eq!(math_types.len(), 2);
    }

    #[test]
    fn test_unregister() {
        let mut db = TypeDatabase::new();
        let id = db.register("TestType", Some("TestCat".to_string()), Some("desc".to_string()));
        assert!(db.get(id).is_some());
        let removed = db.unregister(id);
        assert!(removed.is_some());
        assert!(db.get(id).is_none());
        // Unregistering again should return None
        assert!(db.unregister(id).is_none());
    }

    #[test]
    fn test_clear_and_is_empty() {
        let mut db = TypeDatabase::new();
        db.register("A", None, None);
        db.register("B", None, None);
        assert!(!db.is_empty());
        db.clear();
        assert!(db.is_empty());
        assert_eq!(db.len(), 0);
    }

    #[test]
    fn test_duplicate_names() {
        let mut db = TypeDatabase::new();
        let id1 = db.register("DupType", None, None);
        let id2 = db.register("DupType", None, None);
        let found = db.get_by_name("DupType");
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|t| t.id == id1));
        assert!(found.iter().any(|t| t.id == id2));
    }

    #[test]
    fn test_case_insensitive_lookup() {
        let mut db = TypeDatabase::new();
        db.register("CaseType", Some("Category".to_string()), None);
        let found = db.get_by_name("casetype");
        assert_eq!(found.len(), 1);
        let found_cat = db.get_by_category("category");
        assert_eq!(found_cat.len(), 1);
    }

    #[test]
    fn test_all_returns_all_types() {
        let mut db = TypeDatabase::new();
        db.register("A", None, None);
        db.register("B", None, None);
        let all = db.all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_search_no_results() {
        let mut db = TypeDatabase::new();
        db.register("Alpha", None, None);
        let results = db.search("Beta");
        assert!(results.is_empty());
    }

    #[test]
    fn test_fuzzy_search_no_results() {
        let mut db = TypeDatabase::new();
        db.register("Alpha", None, None);
        let results = db.search_fuzzy("zzz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_large_insert_performance() {
        let mut db = TypeDatabase::new();
        let count = 10_000;
        let start = Instant::now();
        for i in 0..count {
            db.register(format!("Type{}", i), Some("Perf".to_string()), None);
        }
        let duration = start.elapsed();
        assert_eq!(db.len(), count as usize);
        // Should be reasonably fast (arbitrary: < 1s)
        assert!(duration.as_secs_f32() < 1.0, "Insert took too long: {:?}", duration);
    }

    #[test]
    fn test_concurrent_inserts() {
        use std::sync::Mutex;
        let db = Arc::new(Mutex::new(TypeDatabase::new()));
        let threads: Vec<_> = (0..8).map(|t| {
            let db = db.clone();
            thread::spawn(move || {
                for i in 0..2_000 {
                    let mut db = db.lock().unwrap();
                    db.register(format!("T{}_{}", t, i), Some("Cat".to_string()), None);
                }
            })
        }).collect();
        for th in threads { th.join().unwrap(); }
        let db = db.lock().unwrap();
        assert_eq!(db.len(), 16_000);
    }

    #[test]
    fn test_concurrent_reads() {
        let mut db = TypeDatabase::new();
        for i in 0..1000 {
            db.register(format!("Type{}", i), Some("Cat".to_string()), None);
        }
        let db = Arc::new(db);
        let threads: Vec<_> = (0..4).map(|_| {
            let db = db.clone();
            thread::spawn(move || {
                for i in 0..1000 {
                    let _ = db.get_by_name(&format!("Type{}", i));
                }
            })
        }).collect();
        for th in threads { th.join().unwrap(); }
    }
}