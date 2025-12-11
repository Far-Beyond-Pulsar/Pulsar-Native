//! Type Alias Index
//!
//! Maintains an up-to-date index of all type aliases in the project.
//! Provides fast lookups by name and supports searching.

use anyhow::{Result, Context};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use ui_types_common::{AliasAsset, TypeAstNode};

/// Signature of a type alias for quick reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeAliasSignature {
    /// Unique human-readable name (serves as ID)
    pub name: String,
    
    /// Display name for UI
    pub display_name: String,
    
    /// Description of the type
    pub description: String,
    
    /// Path to the source file
    pub file_path: PathBuf,
    
    /// The actual type expression (stored for quick access)
    pub type_expr: String,
    
    /// AST representation (for advanced queries)
    pub ast: Option<TypeAstNode>,
    
    /// Last modified timestamp
    pub last_modified: std::time::SystemTime,
}

impl TypeAliasSignature {
    /// Create from an alias asset and file path
    pub fn from_asset(asset: &AliasAsset, file_path: PathBuf) -> Result<Self> {
        let last_modified = std::fs::metadata(&file_path)?
            .modified()?;
        
        // Generate a human-readable type expression
        let type_expr = Self::ast_to_string(&asset.ast);
        
        Ok(Self {
            name: asset.name.clone(),
            display_name: asset.display_name.clone(),
            description: asset.description.clone().unwrap_or_default(),
            file_path,
            type_expr,
            ast: Some(asset.ast.clone()),
            last_modified,
        })
    }
    
    /// Convert AST to readable string representation
    fn ast_to_string(ast: &TypeAstNode) -> String {
        match ast {
            TypeAstNode::Primitive { name } => name.clone(),
            TypeAstNode::Path { path } => path.clone(),
            TypeAstNode::AliasRef { alias } => alias.clone(),
            TypeAstNode::Constructor { name, params, .. } => {
                let params_str = params
                    .iter()
                    .map(Self::ast_to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", name, params_str)
            }
            TypeAstNode::Tuple { elements } => {
                let elements_str = elements
                    .iter()
                    .map(Self::ast_to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", elements_str)
            }
            TypeAstNode::FnPointer { params, return_type, .. } => {
                let params_str = params
                    .iter()
                    .map(Self::ast_to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("fn({}) -> {}", params_str, Self::ast_to_string(return_type))
            }
            &TypeAstNode::None => String::new(),
        }
    }
    
    /// Check if this signature matches a search query
    pub fn matches_query(&self, query: &str) -> bool {
        let query_lower = query.to_lowercase();
        self.name.to_lowercase().contains(&query_lower)
            || self.display_name.to_lowercase().contains(&query_lower)
            || self.description.to_lowercase().contains(&query_lower)
            || self.type_expr.to_lowercase().contains(&query_lower)
    }
}

/// Index of all type aliases in the project
pub struct TypeAliasIndex {
    /// Map from alias name to signature
    /// Name must be globally unique
    aliases: DashMap<String, TypeAliasSignature>,
}

impl TypeAliasIndex {
    pub fn new() -> Self {
        Self {
            aliases: DashMap::new(),
        }
    }
    
    /// Register or update a type alias from a file
    pub fn register(&self, file_path: &PathBuf) -> Result<()> {
        // Read and parse the file
        let content = std::fs::read_to_string(file_path)
            .context("Failed to read alias file")?;
        
        let asset: AliasAsset = serde_json::from_str(&content)
            .context("Failed to parse alias JSON")?;
        
        // Create signature
        let signature = TypeAliasSignature::from_asset(&asset, file_path.clone())?;
        
        // Check for name conflicts
        if let Some(existing) = self.aliases.get(&signature.name) {
            if existing.file_path != *file_path {
                anyhow::bail!(
                    "Type alias name '{}' conflicts with existing alias at {:?}",
                    signature.name,
                    existing.file_path
                );
            }
        }
        
        // Insert into index
        self.aliases.insert(signature.name.clone(), signature);
        
        Ok(())
    }
    
    /// Unregister a type alias by file path
    pub fn unregister_by_path(&self, file_path: &PathBuf) -> Option<TypeAliasSignature> {
        // Find the alias with this file path
        let name_to_remove = self.aliases
            .iter()
            .find(|entry| &entry.value().file_path == file_path)
            .map(|entry| entry.key().clone());
        
        if let Some(name) = name_to_remove {
            self.aliases.remove(&name).map(|(_, sig)| sig)
        } else {
            None
        }
    }
    
    /// Get a type alias by name
    pub fn get(&self, name: &str) -> Option<TypeAliasSignature> {
        self.aliases.get(name).map(|entry| entry.value().clone())
    }
    
    /// Get all type aliases
    pub fn get_all(&self) -> Vec<TypeAliasSignature> {
        self.aliases
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }
    
    /// Search type aliases by query
    pub fn search(&self, query: &str) -> Vec<TypeAliasSignature> {
        if query.is_empty() {
            return self.get_all();
        }
        
        self.aliases
            .iter()
            .filter(|entry| entry.value().matches_query(query))
            .map(|entry| entry.value().clone())
            .collect()
    }
    
    /// Get count of registered aliases
    pub fn count(&self) -> usize {
        self.aliases.len()
    }
    
    /// Clear all entries
    pub fn clear(&self) {
        self.aliases.clear();
    }
    
    /// Check if a name is available (not in use)
    pub fn is_name_available(&self, name: &str) -> bool {
        !self.aliases.contains_key(name)
    }
    
    /// Validate that a name is unique before saving
    pub fn validate_name(&self, name: &str, file_path: &PathBuf) -> Result<()> {
        if let Some(existing) = self.aliases.get(name) {
            if &existing.file_path != file_path {
                anyhow::bail!(
                    "Type alias name '{}' is already used by {:?}",
                    name,
                    existing.file_path
                );
            }
        }
        Ok(())
    }
}

impl Default for TypeAliasIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_type_alias_index() {
        let index = TypeAliasIndex::new();
        assert_eq!(index.count(), 0);
        assert!(index.is_name_available("MyType"));
    }
}
