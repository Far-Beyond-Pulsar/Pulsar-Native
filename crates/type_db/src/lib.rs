use dashmap::DashMap;

/// Represents a runtime type created by the user in the engine
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

/// An in-memory database for storing and searching user-created runtime types

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
    /// Creates a new empty type database
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new type and returns its assigned ID
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

    /// Removes a type by its ID
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

    /// Gets a type by its ID
    pub fn get(&self, id: u64) -> Option<TypeInfo> {
        self.types.get(&id).map(|v| v.clone())
    }

    /// Gets a type by its exact name
    pub fn get_by_name(&self, name: &str) -> Vec<TypeInfo> {
        self.name_index
            .get(&name.to_lowercase())
            .map(|ids| ids.iter().filter_map(|id| self.types.get(id).map(|v| v.clone())).collect())
            .unwrap_or_default()
    }

    /// Searches for types whose names contain the query string (case-insensitive)
    pub fn search(&self, query: &str) -> Vec<TypeInfo> {
        let query_lower = query.to_lowercase();
        self.types
            .iter()
            .filter(|t| t.name.to_lowercase().contains(&query_lower))
            .map(|t| t.clone())
            .collect()
    }

    /// Searches for types with fuzzy matching on the name
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

    /// Gets all types in a category
    pub fn get_by_category(&self, category: &str) -> Vec<TypeInfo> {
        self.category_index
            .get(&category.to_lowercase())
            .map(|ids| ids.iter().filter_map(|id| self.types.get(id).map(|v| v.clone())).collect())
            .unwrap_or_default()
    }

    /// Returns all registered types
    pub fn all(&self) -> Vec<TypeInfo> {
        self.types.iter().map(|t| t.clone()).collect()
    }

    /// Returns the number of registered types
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Returns true if no types are registered
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Clears all registered types
    pub fn clear(&mut self) {
        self.types.clear();
        self.name_index.clear();
        self.category_index.clear();
    }
}

/// Simple fuzzy matching algorithm that returns a score
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
}