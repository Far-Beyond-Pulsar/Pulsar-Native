use serde::{Deserialize, Serialize};

use crate::identifiers::FileTypeId;

/// Replaced with one RFC 3339 UTC timestamp when a file type's default content
/// is instantiated by the file manager.
pub const CREATION_TIMESTAMP_PLACEHOLDER: &str = "${creation_timestamp}";

// ============================================================================
// File Type Definitions
// ============================================================================

/// Defines the structure of a file type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStructure {
    /// A single standalone file (e.g., script.rs)
    Standalone,

    /// A folder that appears as a file in the drawer (e.g., MyClass.class/)
    /// Contains the marker file name that identifies this folder as this type
    FolderBased {
        /// The marker file that must exist in the folder
        marker_file: String,
        /// Additional files/folders that should be created in a new instance
        template_structure: Vec<PathTemplate>,
    },
}

/// Template for creating files/folders in a folder-based file type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathTemplate {
    /// Create a file with default content
    File { path: String, content: String },
    /// Create a folder
    Folder { path: String },
}

/// Complete definition of a file type that a plugin supports.
#[derive(Debug, Clone)]
pub struct FileTypeDefinition {
    /// Unique identifier for this file type
    pub id: FileTypeId,

    /// File extension (without the dot, e.g., "rs" not ".rs")
    /// For folder-based files, this is the folder extension
    pub extension: String,

    /// Human-readable name for this file type
    pub display_name: String,

    /// Icon to show in the file drawer
    pub icon: ui::IconName,

    /// Color for the icon
    pub color: gpui::Hsla,

    /// Whether this is a standalone file or folder-based
    pub structure: FileStructure,

    /// Default content for new files (as JSON)
    /// For folder-based files, this is the content of the marker file.
    /// `Null` creates an empty file. The empty string marks a type that is
    /// only ever imported or produced, never created from the menu (FBX, PNG,
    /// native meshes); see [`FileTypeDefinition::is_creatable`].
    pub default_content: serde_json::Value,

    /// Optional category path for organizing in the create menu
    /// Examples: vec!["Data"], vec!["Data", "SQLite"], vec!["Scripts", "Web"]
    /// Leave empty for top-level menu items
    pub categories: Vec<String>,

    /// Optional project-relative directory where new assets of this type are
    /// created, regardless of the folder currently selected in the file manager.
    /// This is useful for project assets with a canonical location, such as
    /// type definitions under `types/traits`.
    pub creation_directory: Option<String>,
}

impl FileTypeDefinition {
    /// Whether the file manager offers to create files of this type. Types
    /// whose files are imported or produced by a tool (FBX, PNG, `.mesh`)
    /// register with an empty-string `default_content`, which would only
    /// write an invalid file.
    pub fn is_creatable(&self) -> bool {
        self.default_content != serde_json::Value::String(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_content(default_content: serde_json::Value) -> FileTypeDefinition {
        FileTypeDefinition {
            id: FileTypeId::new("x"),
            extension: "x".into(),
            display_name: "X".into(),
            icon: ui::IconName::Cube,
            color: gpui::Hsla::default(),
            structure: FileStructure::Standalone,
            default_content,
            categories: Vec::new(),
            creation_directory: None,
        }
    }

    #[test]
    fn only_import_only_types_are_hidden_from_the_create_menu() {
        assert!(!with_content(serde_json::json!("")).is_creatable());
        assert!(with_content(serde_json::Value::Null).is_creatable(), "an empty file");
        assert!(with_content(serde_json::json!("# notes\n")).is_creatable());
        assert!(with_content(serde_json::json!({ "nodes": [] })).is_creatable());
    }
}
