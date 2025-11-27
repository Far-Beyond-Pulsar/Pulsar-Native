use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::{TypeKind, TypeSystemError, Result};

/// Metadata for a type in the index
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TypeIndexEntry {
    pub id: String,
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// Relative path from project root to type directory
    pub path: String,
    #[serde(rename = "modPath")]
    pub mod_path: String,
    #[serde(rename = "jsonFile")]
    pub json_file: String,
    #[serde(rename = "rsFile")]
    pub rs_file: String,
    #[serde(rename = "lastCompiled")]
    pub last_compiled: String,
    pub version: u32,
}

impl TypeIndexEntry {
    pub fn new(kind: TypeKind, name: String, display_name: String) -> Self {
        let type_dir = format!("types/{}/{}", kind.plural(), name);
        let json_filename = format!("{}.json", kind.as_str());

        Self {
            id: format!("{}:{}", kind.as_str(), name),
            name: name.clone(),
            display_name,
            path: format!("../{}", type_dir),
            mod_path: format!("crate::types::{}::{}::mod", kind.plural(), name),
            json_file: format!("{}/{}", type_dir, json_filename),
            rs_file: format!("{}/mod.rs", type_dir),
            last_compiled: chrono::Utc::now().to_rfc3339(),
            version: 1,
        }
    }

    pub fn increment_version(&mut self) {
        self.version += 1;
        self.last_compiled = chrono::Utc::now().to_rfc3339();
    }
}

/// Central type index
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TypeIndex {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    pub types: TypeIndexTypes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TypeIndexTypes {
    pub aliases: HashMap<String, TypeIndexEntry>,
    pub structs: HashMap<String, TypeIndexEntry>,
    pub enums: HashMap<String, TypeIndexEntry>,
    pub traits: HashMap<String, TypeIndexEntry>,
}

impl Default for TypeIndex {
    fn default() -> Self {
        Self {
            schema_version: 1,
            generated_at: chrono::Utc::now().to_rfc3339(),
            types: TypeIndexTypes {
                aliases: HashMap::new(),
                structs: HashMap::new(),
                enums: HashMap::new(),
                traits: HashMap::new(),
            },
        }
    }
}

impl TypeIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load index from file
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Save index to file
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.generated_at = chrono::Utc::now().to_rfc3339();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Add or update a type in the index
    pub fn upsert(&mut self, kind: TypeKind, entry: TypeIndexEntry) -> Result<()> {
        let map = self.get_map_mut(kind);
        map.insert(entry.name.clone(), entry);
        Ok(())
    }

    /// Remove a type from the index
    pub fn remove(&mut self, kind: TypeKind, name: &str) -> Result<()> {
        let map = self.get_map_mut(kind);
        map.remove(name);
        Ok(())
    }

    /// Get a type entry
    pub fn get(&self, kind: TypeKind, name: &str) -> Option<&TypeIndexEntry> {
        self.get_map(kind).get(name)
    }

    /// Check if a type exists
    pub fn exists(&self, kind: TypeKind, name: &str) -> bool {
        self.get_map(kind).contains_key(name)
    }

    /// Check for name collisions across all type kinds
    pub fn has_collision(&self, name: &str) -> Option<TypeKind> {
        if self.types.structs.contains_key(name) {
            Some(TypeKind::Struct)
        } else if self.types.enums.contains_key(name) {
            Some(TypeKind::Enum)
        } else if self.types.traits.contains_key(name) {
            Some(TypeKind::Trait)
        } else if self.types.aliases.contains_key(name) {
            Some(TypeKind::Alias)
        } else {
            None
        }
    }

    /// Get all aliases (for type pickers)
    pub fn get_all_aliases(&self) -> Vec<&TypeIndexEntry> {
        self.types.aliases.values().collect()
    }

    /// Get all types of a specific kind
    pub fn get_all(&self, kind: TypeKind) -> Vec<&TypeIndexEntry> {
        self.get_map(kind).values().collect()
    }

    fn get_map(&self, kind: TypeKind) -> &HashMap<String, TypeIndexEntry> {
        match kind {
            TypeKind::Struct => &self.types.structs,
            TypeKind::Enum => &self.types.enums,
            TypeKind::Trait => &self.types.traits,
            TypeKind::Alias => &self.types.aliases,
        }
    }

    fn get_map_mut(&mut self, kind: TypeKind) -> &mut HashMap<String, TypeIndexEntry> {
        match kind {
            TypeKind::Struct => &mut self.types.structs,
            TypeKind::Enum => &mut self.types.enums,
            TypeKind::Trait => &mut self.types.traits,
            TypeKind::Alias => &mut self.types.aliases,
        }
    }
}

/// Manager for type index operations
pub struct IndexManager {
    project_root: PathBuf,
    index_path: PathBuf,
}

impl IndexManager {
    pub fn new(project_root: PathBuf) -> Self {
        let index_path = project_root.join("type-index").join("index.json");
        Self {
            project_root,
            index_path,
        }
    }

    pub fn load_index(&self) -> Result<TypeIndex> {
        TypeIndex::load(&self.index_path)
    }

    pub fn save_index(&self, index: &mut TypeIndex) -> Result<()> {
        index.save(&self.index_path)
    }

    pub fn get_type_dir(&self, kind: TypeKind, name: &str) -> PathBuf {
        self.project_root
            .join("types")
            .join(kind.plural())
            .join(name)
    }

    pub fn get_json_path(&self, kind: TypeKind, name: &str) -> PathBuf {
        self.get_type_dir(kind, name)
            .join(format!("{}.json", kind.as_str()))
    }

    pub fn get_rs_path(&self, kind: TypeKind, name: &str) -> PathBuf {
        self.get_type_dir(kind, name).join("mod.rs")
    }

    pub fn ensure_type_dir(&self, kind: TypeKind, name: &str) -> Result<PathBuf> {
        let dir = self.get_type_dir(kind, name);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}
