//! # Asset Index
//!
//! In-memory, thread-safe index of project assets discovered by [`crate::scanner::ProjectScanner`].
//! Supports fast registration and lookup by ID, name, category, file path, or file type.

use dashmap::DashMap;
use plugin_editor_api::FileTypeId;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Information about a single project asset file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetInfo {
    /// Unique identifier for the asset
    pub id: u64,
    /// Display name of the asset
    pub name: String,
    /// Optional category for grouping assets
    pub category: Option<String>,
    /// Optional description of the asset
    pub description: Option<String>,
    /// File path where this asset is located
    pub file_path: Option<PathBuf>,
    /// File type ID from the plugin registry (e.g., "struct", "enum", "trait", "alias", "scene")
    pub file_type_id: FileTypeId,
    /// Display name for UI (may differ from name)
    pub display_name: String,
    /// Last modified timestamp
    pub last_modified: Option<SystemTime>,
}

/// An in-memory, thread-safe index of project asset files.
///
/// `AssetIndex` supports fast registration, removal, and lookup of assets by ID, name, or category.
/// It uses `DashMap` internally for concurrent access, making it suitable for multi-threaded environments.
#[derive(Debug)]
pub struct AssetIndex {
    /// Map of asset ID to asset info
    assets: DashMap<u64, AssetInfo>,
    /// Index for name-based lookups (lowercase name -> asset IDs)
    name_index: DashMap<String, Vec<u64>>,
    /// Index for category-based lookups
    category_index: DashMap<String, Vec<u64>>,
    /// Index for file path-based lookups (file path -> asset ID)
    file_path_index: DashMap<PathBuf, u64>,
    /// Next available asset ID (atomic for interior mutability)
    next_id: AtomicU64,
}

impl Default for AssetIndex {
    fn default() -> Self {
        Self {
            assets: DashMap::new(),
            name_index: DashMap::new(),
            category_index: DashMap::new(),
            file_path_index: DashMap::new(),
            next_id: AtomicU64::new(0),
        }
    }
}

impl AssetIndex {
    /// Creates a new, empty asset index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new asset with all available fields and returns its assigned unique ID.
    pub fn register(
        &self,
        name: impl Into<String>,
        category: Option<String>,
        description: Option<String>,
        file_path: Option<PathBuf>,
        file_type_id: FileTypeId,
        display_name: Option<String>,
        last_modified: Option<SystemTime>,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        let name = name.into();
        let display_name = display_name.unwrap_or_else(|| name.clone());

        let asset_info = AssetInfo {
            id,
            name: name.clone(),
            category: category.clone(),
            description,
            file_path: file_path.clone(),
            file_type_id,
            display_name,
            last_modified,
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

        // Add to file path index
        if let Some(path) = &file_path {
            self.file_path_index.insert(path.clone(), id);
        }

        self.assets.insert(id, asset_info);
        id
    }

    /// Registers an asset without all optional fields.
    pub fn register_simple(&self, name: impl Into<String>, file_type_id: FileTypeId) -> u64 {
        self.register(name, None, None, None, file_type_id, None, None)
    }

    /// Registers an asset with file path (common case for engine_fs).
    ///
    /// This method automatically extracts the last_modified timestamp from the file system.
    pub fn register_with_path(
        &self,
        name: impl Into<String>,
        file_path: PathBuf,
        file_type_id: FileTypeId,
        display_name: Option<String>,
        description: Option<String>,
    ) -> Result<u64, String> {
        // Get file metadata for last_modified
        let last_modified = std::fs::metadata(&file_path)
            .ok()
            .and_then(|m| m.modified().ok());

        Ok(self.register(
            name,
            None,
            description,
            Some(file_path),
            file_type_id,
            display_name,
            last_modified,
        ))
    }

    /// Removes an asset by its ID.
    pub fn unregister(&self, id: u64) -> Option<AssetInfo> {
        if let Some((_, asset_info)) = self.assets.remove(&id) {
            // Remove from name index
            if let Some(mut ids) = self.name_index.get_mut(&asset_info.name.to_lowercase()) {
                ids.retain(|&i| i != id);
            }

            // Remove from category index
            if let Some(cat) = &asset_info.category {
                if let Some(mut ids) = self.category_index.get_mut(&cat.to_lowercase()) {
                    ids.retain(|&i| i != id);
                }
            }

            // Remove from file path index
            if let Some(path) = &asset_info.file_path {
                self.file_path_index.remove(path);
            }

            Some(asset_info)
        } else {
            None
        }
    }

    /// Gets an asset by file path.
    pub fn get_by_path(&self, file_path: &PathBuf) -> Option<AssetInfo> {
        self.file_path_index
            .get(file_path)
            .and_then(|id| self.assets.get(&id).map(|v| v.clone()))
    }

    /// Unregisters an asset by file path.
    pub fn unregister_by_path(&self, file_path: &PathBuf) -> Option<AssetInfo> {
        if let Some((_, id)) = self.file_path_index.remove(file_path) {
            self.unregister(id)
        } else {
            None
        }
    }

    /// Gets all assets of a specific file type.
    pub fn get_by_file_type(&self, file_type_id: &FileTypeId) -> Vec<AssetInfo> {
        self.assets
            .iter()
            .filter(|t| &t.file_type_id == file_type_id)
            .map(|t| t.clone())
            .collect()
    }

    /// Gets the count of assets of a specific file type.
    pub fn count_by_file_type(&self, file_type_id: &FileTypeId) -> usize {
        self.assets
            .iter()
            .filter(|t| &t.file_type_id == file_type_id)
            .count()
    }

    /// Gets an asset by its unique ID.
    pub fn get(&self, id: u64) -> Option<AssetInfo> {
        self.assets.get(&id).map(|v| v.clone())
    }

    /// Gets all assets with the given exact name (case-insensitive).
    pub fn get_by_name(&self, name: &str) -> Vec<AssetInfo> {
        self.name_index
            .get(&name.to_lowercase())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.assets.get(id).map(|v| v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Searches for assets whose names contain the query string (case-insensitive substring match).
    pub fn search(&self, query: &str) -> Vec<AssetInfo> {
        let query_lower = query.to_lowercase();
        self.assets
            .iter()
            .filter(|t| t.name.to_lowercase().contains(&query_lower))
            .map(|t| t.clone())
            .collect()
    }

    /// Searches for assets with fuzzy matching on the name.
    pub fn search_fuzzy(&self, query: &str) -> Vec<AssetInfo> {
        let query_lower = query.to_lowercase();
        let query_chars: Vec<char> = query_lower.chars().collect();

        let mut results: Vec<(AssetInfo, i32)> = self
            .assets
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

    /// Gets all assets in a given category (case-insensitive).
    pub fn get_by_category(&self, category: &str) -> Vec<AssetInfo> {
        self.category_index
            .get(&category.to_lowercase())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.assets.get(id).map(|v| v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all registered assets in the index.
    pub fn all(&self) -> Vec<AssetInfo> {
        self.assets.iter().map(|t| t.clone()).collect()
    }

    /// Returns the number of registered assets in the index.
    pub fn len(&self) -> usize {
        self.assets.len()
    }

    /// Returns `true` if no assets are registered in the index.
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }

    /// Clears all registered assets from the index.
    pub fn clear(&self) {
        self.assets.clear();
        self.name_index.clear();
        self.category_index.clear();
        self.file_path_index.clear();
        self.next_id.store(0, Ordering::SeqCst);
    }
}

/// Simple fuzzy matching algorithm that returns a score.
///
/// Returns a positive score if all pattern characters are found in order in the text.
/// Higher scores are given for consecutive matches and matches at word/segment boundaries.
/// Returns 0 if the pattern is not fully matched.
pub(crate) fn fuzzy_match(pattern: &[char], text: &str) -> i32 {
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
