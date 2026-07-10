//! # User Type Registry
//!
//! Tracks user-defined type aliases (`.alias.json` / [`ui_types_common::AliasAsset`]) and
//! registers them into [`pulsar_reflection`]'s global runtime type registries
//! ([`pulsar_reflection::RUNTIME_TYPE_REGISTRY`] for compile-time base types,
//! [`pulsar_reflection::DYNAMIC_TYPE_REGISTRY`] for the user-defined types themselves).
//!
//! Each alias is represented as a single-field [`pulsar_reflection::DynamicTypeInfo`]
//! wrapping the resolved base type, which lets `pulsar_reflection` remain the sole
//! source of truth for type information while this module just maintains the
//! file-path/name <-> type bookkeeping needed by the project filesystem.

use crate::asset_index::fuzzy_match;
use crate::{events, FsChangeKind};
use anyhow::{Context, Result};
use dashmap::DashMap;
use plugin_editor_api::FileTypeId;
use pulsar_reflection::{
    DynamicTypeBuilder, DynamicTypeInfo, RuntimeTypeInfo, DYNAMIC_TYPE_REGISTRY,
    RUNTIME_TYPE_REGISTRY,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use ui_types_common::types::TypeAstNode;
use uuid::Uuid;

/// Metadata about a user-defined type alias, kept alongside the
/// [`DynamicTypeInfo`] registered in [`DYNAMIC_TYPE_REGISTRY`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserTypeInfo {
    /// UUID of the registered [`DynamicTypeInfo`] in [`DYNAMIC_TYPE_REGISTRY`]
    pub uuid: Uuid,
    /// Name of the type alias
    pub name: String,
    /// Display name for UI
    pub display_name: String,
    /// Optional description
    pub description: Option<String>,
    /// File path where this alias is defined
    pub file_path: PathBuf,
    /// File type ID from the plugin registry (always "alias" for now)
    pub file_type_id: FileTypeId,
    /// Last modified timestamp
    pub last_modified: Option<SystemTime>,
}

/// Registry of user-defined type aliases, indexed by UUID, name, and file path.
///
/// The actual type information lives in [`DYNAMIC_TYPE_REGISTRY`]; this registry
/// just tracks the file-system-facing metadata and keeps it in sync.
#[derive(Debug, Default)]
pub struct UserTypeRegistry {
    by_uuid: DashMap<Uuid, UserTypeInfo>,
    by_path: DashMap<PathBuf, Uuid>,
    by_name: DashMap<String, Uuid>,
}

impl UserTypeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns all registered user types.
    pub fn all(&self) -> Vec<UserTypeInfo> {
        self.by_uuid.iter().map(|e| e.value().clone()).collect()
    }

    /// Looks up a user type by name (case-insensitive).
    pub fn get_by_name(&self, name: &str) -> Option<UserTypeInfo> {
        let uuid = self.by_name.get(&name.to_lowercase())?;
        self.by_uuid.get(uuid.value()).map(|e| e.value().clone())
    }

    /// Looks up a user type by file path.
    pub fn get_by_path(&self, file_path: &Path) -> Option<UserTypeInfo> {
        let uuid = self.by_path.get(file_path)?;
        self.by_uuid.get(uuid.value()).map(|e| e.value().clone())
    }

    /// Returns all user types with the given file type ID.
    pub fn get_by_file_type(&self, file_type_id: &FileTypeId) -> Vec<UserTypeInfo> {
        self.by_uuid
            .iter()
            .filter(|e| &e.value().file_type_id == file_type_id)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Searches for user types whose names contain the query string (case-insensitive).
    pub fn search(&self, query: &str) -> Vec<UserTypeInfo> {
        let query_lower = query.to_lowercase();
        self.by_uuid
            .iter()
            .filter(|e| e.value().name.to_lowercase().contains(&query_lower))
            .map(|e| e.value().clone())
            .collect()
    }

    /// Searches for user types with fuzzy matching on the name.
    pub fn search_fuzzy(&self, query: &str) -> Vec<UserTypeInfo> {
        let query_lower = query.to_lowercase();
        let query_chars: Vec<char> = query_lower.chars().collect();

        let mut results: Vec<(UserTypeInfo, i32)> = self
            .by_uuid
            .iter()
            .filter_map(|e| {
                let info = e.value().clone();
                let score = fuzzy_match(&query_chars, &info.name.to_lowercase());
                if score > 0 {
                    Some((info, score))
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.1.cmp(&a.1));
        results.into_iter().map(|(t, _)| t).collect()
    }

    pub fn len(&self) -> usize {
        self.by_uuid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_uuid.is_empty()
    }

    /// Removes all user types from this registry and from [`DYNAMIC_TYPE_REGISTRY`].
    pub fn clear(&self) {
        for entry in self.by_uuid.iter() {
            DYNAMIC_TYPE_REGISTRY.unregister(&entry.value().uuid);
        }
        self.by_uuid.clear();
        self.by_path.clear();
        self.by_name.clear();
    }

    /// Removes a user type by file path, unregistering it from [`DYNAMIC_TYPE_REGISTRY`] too.
    pub fn unregister_by_path(&self, file_path: &Path) -> Option<UserTypeInfo> {
        let (_, uuid) = self.by_path.remove(file_path)?;
        let (_, info) = self.by_uuid.remove(&uuid)?;
        self.by_name.remove(&info.name.to_lowercase());
        DYNAMIC_TYPE_REGISTRY.unregister(&uuid);
        Some(info)
    }

    /// Reads, parses, and registers a `.alias.json` file, building a [`DynamicTypeInfo`]
    /// for it and registering it in [`DYNAMIC_TYPE_REGISTRY`].
    ///
    /// If a type was already registered for this path, it is replaced.
    pub fn register_alias_file(&self, file_path: &Path) -> Result<Uuid> {
        let content = std::fs::read_to_string(file_path).context("Failed to read alias file")?;
        let asset: ui_types_common::AliasAsset =
            serde_json::from_str(&content).context("Invalid alias JSON")?;
        self.register_alias_asset(file_path.to_path_buf(), asset, true)
    }

    /// Registers an already-parsed [`ui_types_common::AliasAsset`] for the given path.
    ///
    /// If `check_unique` is true, fails if another file already registered a type with
    /// the same name.
    fn register_alias_asset(
        &self,
        file_path: PathBuf,
        asset: ui_types_common::AliasAsset,
        check_unique: bool,
    ) -> Result<Uuid> {
        if check_unique {
            if let Some(existing) = self.get_by_name(&asset.name) {
                if existing.file_path != file_path {
                    anyhow::bail!("Type alias name '{}' is already in use", asset.name);
                }
            }
        }

        // Replace any previous registration for this path
        self.unregister_by_path(&file_path);

        let base_type = self.resolve_ast(&asset.ast);
        let dynamic_type = DynamicTypeBuilder::new(asset.name.clone())
            .add_field("value", base_type)
            .build();
        let uuid = DYNAMIC_TYPE_REGISTRY.register(dynamic_type);

        let last_modified = std::fs::metadata(&file_path)
            .ok()
            .and_then(|m| m.modified().ok());

        let info = UserTypeInfo {
            uuid,
            name: asset.name.clone(),
            display_name: asset.display_name,
            description: asset.description,
            file_path: file_path.clone(),
            file_type_id: FileTypeId::new("alias"),
            last_modified,
        };

        self.by_name.insert(asset.name.to_lowercase(), uuid);
        self.by_path.insert(file_path, uuid);
        self.by_uuid.insert(uuid, info);

        Ok(uuid)
    }

    /// Resolves a [`TypeAstNode`] to a `&'static RuntimeTypeInfo`.
    ///
    /// `Primitive`/`Path` nodes resolve via [`RUNTIME_TYPE_REGISTRY`]. `AliasRef` nodes
    /// resolve transitively through other registered user types. Anything else
    /// (constructors, tuples, function pointers, or unresolvable names) falls back to
    /// [`RuntimeTypeInfo::wildcard`].
    fn resolve_ast(&self, node: &TypeAstNode) -> &'static RuntimeTypeInfo {
        match node {
            TypeAstNode::Primitive { name } | TypeAstNode::Path { path: name } => {
                RUNTIME_TYPE_REGISTRY
                    .get_by_name(name)
                    .unwrap_or_else(RuntimeTypeInfo::wildcard)
            }
            TypeAstNode::AliasRef { alias } => self
                .get_by_name(alias)
                .and_then(|info| DYNAMIC_TYPE_REGISTRY.get(&info.uuid))
                .and_then(|dynamic_type| {
                    dynamic_type.get_field("value").map(|field| field.base_type)
                })
                .unwrap_or_else(RuntimeTypeInfo::wildcard),
            TypeAstNode::None
            | TypeAstNode::Constructor { .. }
            | TypeAstNode::Tuple { .. }
            | TypeAstNode::FnPointer { .. } => RuntimeTypeInfo::wildcard(),
        }
    }

    /// Creates a new type alias file under `<project_root>/types/aliases/`.
    pub fn create_type_alias(
        &self,
        project_root: &Path,
        name: &str,
        content: &str,
    ) -> Result<PathBuf> {
        if self.get_by_name(name).is_some() {
            anyhow::bail!("Type alias name '{}' is already in use", name);
        }

        let file_path = project_root
            .join("types")
            .join("aliases")
            .join(format!("{}.alias.json", name));

        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&file_path, content).context("Failed to write alias file")?;

        if let Err(e) = self.register_alias_file(&file_path) {
            tracing::warn!("Failed to register type alias '{}': {:?}", name, e);
        }

        Ok(file_path)
    }

    /// Updates an existing type alias file, re-registering it.
    pub fn update_type_alias(&self, file_path: &Path, content: &str) -> Result<()> {
        let asset: ui_types_common::AliasAsset =
            serde_json::from_str(content).context("Invalid alias JSON")?;

        if let Some(existing) = self.get_by_name(&asset.name) {
            if existing.file_path != file_path {
                anyhow::bail!("Type alias name '{}' is already in use", asset.name);
            }
        }

        std::fs::write(file_path, content).context("Failed to write alias file")?;

        if let Err(e) = self.register_alias_asset(file_path.to_path_buf(), asset.clone(), true) {
            tracing::warn!("Failed to update type alias '{}': {:?}", asset.name, e);
        }
        events::emit(file_path.to_path_buf(), FsChangeKind::Modified);

        Ok(())
    }

    /// Deletes a type alias file and unregisters it.
    pub fn delete_type_alias(&self, file_path: &Path) -> Result<()> {
        self.unregister_by_path(file_path);

        std::fs::remove_file(file_path).context("Failed to delete alias file")?;
        events::emit(file_path.to_path_buf(), FsChangeKind::Deleted);

        Ok(())
    }

    /// Moves/renames a type alias file and re-registers it at the new location.
    pub fn move_type_alias(&self, old_path: &Path, new_path: &Path) -> Result<()> {
        self.unregister_by_path(old_path);

        std::fs::rename(old_path, new_path).context("Failed to move alias file")?;

        if let Err(e) = self.register_alias_file(new_path) {
            tracing::warn!(
                "Failed to register renamed type alias at {:?}: {:?}",
                new_path,
                e
            );
        }
        events::emit(old_path.to_path_buf(), FsChangeKind::Deleted);
        events::emit(new_path.to_path_buf(), FsChangeKind::Created);

        Ok(())
    }
}

// Re-exported for convenience so callers don't need to depend on pulsar_reflection directly
// just to reference the underlying dynamic type info.
pub use pulsar_reflection::DynamicTypeInfo as UserDynamicTypeInfo;

/// Looks up the [`DynamicTypeInfo`] for a registered user type by UUID.
pub fn get_dynamic_type(uuid: &Uuid) -> Option<Arc<DynamicTypeInfo>> {
    DYNAMIC_TYPE_REGISTRY.get(uuid)
}
