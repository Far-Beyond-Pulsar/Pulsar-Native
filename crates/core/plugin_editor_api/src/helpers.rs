use crate::file_types::{FileStructure, FileTypeDefinition, PathTemplate};
use crate::identifiers::FileTypeId;

/// Helper to create a simple standalone file type definition.
pub fn standalone_file_type(
    id: impl Into<String>,
    extension: impl Into<String>,
    display_name: impl Into<String>,
    icon: ui::IconName,
    color: gpui::Hsla,
    default_content: serde_json::Value,
) -> FileTypeDefinition {
    FileTypeDefinition {
        id: FileTypeId::new(id),
        extension: extension.into(),
        display_name: display_name.into(),
        icon,
        color,
        structure: FileStructure::Standalone,
        default_content,
        categories: vec![],
    }
}

/// Helper to create a folder-based file type definition.
pub fn folder_file_type(
    id: impl Into<String>,
    extension: impl Into<String>,
    display_name: impl Into<String>,
    icon: ui::IconName,
    color: gpui::Hsla,
    marker_file: impl Into<String>,
    template_structure: Vec<PathTemplate>,
    default_content: serde_json::Value,
) -> FileTypeDefinition {
    FileTypeDefinition {
        id: FileTypeId::new(id),
        extension: extension.into(),
        display_name: display_name.into(),
        icon,
        color,
        structure: FileStructure::FolderBased {
            marker_file: marker_file.into(),
            template_structure,
        },
        default_content,
        categories: vec![],
    }
}
