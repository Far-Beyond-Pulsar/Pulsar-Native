use gpui::*;
use std::path::{Path, PathBuf};
use ui::ActiveTheme;

// ============================================================================
// ENUMS - View modes, sort options, drag state
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewMode {
    Grid,
    List,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortBy {
    Name,
    Modified,
    Size,
    Type,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragState {
    None,
    Dragging {
        source_paths: Vec<PathBuf>,
        is_folder: bool,
        drag_offset: Point<Pixels>,
    },
}

// Drag data for file items
#[derive(Clone, Debug)]
pub struct DraggedFile {
    pub paths: Vec<PathBuf>,
    pub is_folder: bool,
    pub drag_start_position: Option<Point<Pixels>>,
}

impl gpui::Render for DraggedFile {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::prelude::*;

        let count = self.paths.len();
        let label = if count == 1 {
            self.paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string()
        } else {
            format!("{} items", count)
        };

        gpui::div()
            .px_3()
            .py_1p5()
            .rounded(px(6.0))
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
            .text_sm()
            .shadow_lg()
            .child(label)
    }
}

// ============================================================================
// FILE ITEM - Represents a file or folder in the file system
// ============================================================================

#[derive(Clone, Debug)]
pub struct FileItem {
    pub path: PathBuf,
    pub name: String,
    pub file_type_def: Option<plugin_editor_api::FileTypeDefinition>,
    pub is_folder: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

impl FileItem {
    pub fn from_path(
        path: &Path,
        file_types: &[plugin_editor_api::FileTypeDefinition],
    ) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_string();

        let file_type_def = if path.is_dir() {
            // Check registry for folder-based file types
            file_types.iter().find(|def| {
                if let plugin_editor_api::FileStructure::FolderBased { marker_file, .. } = &def.structure {
                    path.join(marker_file).exists()
                } else {
                    false
                }
            }).cloned()
        } else {
            // Check registry for standalone file types
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            
            // Try full file name first (for compound extensions like .level.json)
            let mut found = file_types.iter().find(|def| {
                if matches!(def.structure, plugin_editor_api::FileStructure::Standalone) {
                    file_name.ends_with(&format!(".{}", def.extension))
                } else {
                    false
                }
            });
            
            // If not found, try simple extension match
            if found.is_none() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    found = file_types.iter().find(|def| {
                        matches!(def.structure, plugin_editor_api::FileStructure::Standalone) && def.extension == ext
                    });
                }
            }
            
            found.cloned()
        };

        let metadata = std::fs::metadata(path).ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata.and_then(|m| m.modified().ok());

        let is_folder = path.is_dir() && file_type_def.is_none();

        Some(FileItem {
            path: path.to_path_buf(),
            name,
            file_type_def,
            is_folder,
            size,
            modified,
        })
    }

    pub fn display_name(&self) -> &str {
        self.file_type_def.as_ref()
            .map(|def| def.display_name.as_str())
            .unwrap_or(if self.is_folder { "Folder" } else { "File" })
    }

    pub fn is_class(&self) -> bool {
        self.file_type_def.as_ref()
            .map(|def| def.id.as_str() == "class")
            .unwrap_or(false)
    }
}

// ============================================================================
// EVENTS - File manager events
// ============================================================================

#[derive(Clone, Debug)]
pub struct FileSelected {
    pub path: PathBuf,
    pub file_type_def: Option<plugin_editor_api::FileTypeDefinition>,
}

#[derive(Clone, Debug)]
pub struct PopoutFileManagerEvent {
    pub position: Point<Pixels>,
}
