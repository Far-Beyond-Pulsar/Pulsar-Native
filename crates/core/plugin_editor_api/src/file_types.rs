use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::identifiers::FileTypeId;
use crate::scripting::CookOptimization;

/// Inputs supplied to a file type's cook callback.
pub struct CookContext<'a> {
    pub project_root: &'a Path,
    pub source_path: &'a Path,
    /// Package-relative destination assigned by the host (normally `assets/...`).
    pub content_path: &'a Path,
    pub optimization: CookOptimization,
}

/// One artifact emitted by a file type's cook callback.
#[derive(Clone, Debug)]
pub struct CookedFile {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

/// The package-ready files emitted by a cook handler and its direct asset
/// references. Hosts follow those references to a fixed point, so handlers
/// only need to report immediate dependencies.
#[derive(Clone, Debug, Default)]
pub struct CookOutput {
    pub files: Vec<CookedFile>,
    pub dependencies: Vec<String>,
}

impl CookOutput {
    pub fn single(file: CookedFile) -> Self {
        Self {
            files: vec![file],
            dependencies: Vec::new(),
        }
    }

    pub fn new(files: Vec<CookedFile>, dependencies: Vec<String>) -> Self {
        Self {
            files,
            dependencies,
        }
    }
}

/// Converts a file of this type into package-ready artifacts and direct
/// references.
pub type CookCallback = for<'a> fn(&CookContext<'a>) -> Result<CookOutput, String>;

/// Copy formats whose authored file is already runtime-ready.
pub fn copy_file_for_cook(context: &CookContext<'_>) -> Result<CookOutput, String> {
    let bytes = engine_fs::virtual_fs::read_file(context.source_path)
        .map_err(|error| format!("{}: {error}", context.source_path.display()))?;
    Ok(CookOutput::single(CookedFile {
        path: context.content_path.to_path_buf(),
        bytes,
    }))
}

/// Copy every file in a folder-based asset while keeping paths relative to
/// the asset root. The dependency callback registered for that file type
/// reports references contained in its files.
pub fn copy_folder_for_cook(context: &CookContext<'_>) -> Result<CookOutput, String> {
    fn visit(
        source_root: &Path,
        current: &Path,
        content_root: &Path,
        files: &mut Vec<CookedFile>,
    ) -> Result<(), String> {
        let entries = engine_fs::virtual_fs::list_dir(current)
            .map_err(|error| format!("{}: {error}", current.display()))?;
        for entry in entries {
            let source = current.join(&entry.name);
            if entry.is_dir {
                visit(source_root, &source, content_root, files)?;
                continue;
            }
            let relative = source
                .strip_prefix(source_root)
                .map_err(|error| format!("{}: {error}", source.display()))?;
            let bytes = engine_fs::virtual_fs::read_file(&source)
                .map_err(|error| format!("{}: {error}", source.display()))?;
            files.push(CookedFile {
                path: content_root.join(relative),
                bytes,
            });
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(
        context.source_path,
        context.source_path,
        context.content_path,
        &mut files,
    )?;
    if files.is_empty() {
        return Err(format!(
            "{} contains no files to cook",
            context.source_path.display()
        ));
    }
    Ok(CookOutput::new(files, Vec::new()))
}

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

    /// Icon to show in the file drawer.
    #[cfg(feature = "editor")]
    pub icon: ui::IconName,

    /// Color for the icon.
    #[cfg(feature = "editor")]
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

    /// Plugin-owned cook handler. Choose `copy_file_for_cook` for a runtime
    /// ready format, or provide the transformation owned by this file type.
    pub cook: CookCallback,
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
            #[cfg(feature = "editor")]
            icon: ui::IconName::Cube,
            #[cfg(feature = "editor")]
            color: gpui::Hsla::default(),
            structure: FileStructure::Standalone,
            default_content,
            categories: Vec::new(),
            creation_directory: None,
            cook: copy_file_for_cook,
        }
    }

    #[test]
    fn only_import_only_types_are_hidden_from_the_create_menu() {
        assert!(!with_content(serde_json::json!("")).is_creatable());
        assert!(
            with_content(serde_json::Value::Null).is_creatable(),
            "an empty file"
        );
        assert!(with_content(serde_json::json!("# notes\n")).is_creatable());
        assert!(with_content(serde_json::json!({ "nodes": [] })).is_creatable());
    }
}
