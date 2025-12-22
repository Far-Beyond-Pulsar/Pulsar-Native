use gpui::*;
use std::path::{Path, PathBuf};

// ============================================================================
// ENUMS - View modes, sort options, drag state
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum FileType {
    Folder,
    Class, // A folder containing graph_save.json
    Script,
    DawProject, // .pdaw files
    LevelScene, // .level.json files
    Database, // .db, .sqlite, .sqlite3 files
    Config, // .toml files
    // Type System Files
    StructType,  // .struct.json files
    EnumType,    // .enum.json files
    TraitType,   // .trait.json files
    AliasType,   // .alias.json files
    Image,       // .png, .jpg, .jpeg, .gif, .bmp, .svg
    Audio,       // .wav, .mp3, .ogg, .flac
    Video,       // .mp4, .webm, .avi
    Document,    // .txt, .md, .pdf
    Archive,     // .zip, .tar, .gz, .7z
    Other,
}

impl FileType {
    /// Check if this file type is a class (folder with graph_save.json)
    pub fn is_class(&self) -> bool {
        matches!(self, FileType::Class)
    }

    /// Get display name for the file type
    pub fn display_name(&self) -> &'static str {
        match self {
            FileType::Folder => "Folder",
            FileType::Class => "Class",
            FileType::Script => "Script",
            FileType::DawProject => "DAW Project",
            FileType::LevelScene => "Level Scene",
            FileType::Database => "Database",
            FileType::Config => "Config",
            FileType::StructType => "Struct",
            FileType::EnumType => "Enum",
            FileType::TraitType => "Trait",
            FileType::AliasType => "Alias",
            FileType::Image => "Image",
            FileType::Audio => "Audio",
            FileType::Video => "Video",
            FileType::Document => "Document",
            FileType::Archive => "Archive",
            FileType::Other => "File",
        }
    }
}

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

// ============================================================================
// FILE ITEM - Represents a file or folder in the file system
// ============================================================================

#[derive(Clone, Debug)]
pub struct FileItem {
    pub path: PathBuf,
    pub name: String,
    pub file_type: FileType,
    pub is_folder: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

impl FileItem {
    pub fn is_class_folder(path: &Path) -> bool {
        path.is_dir() && path.join("graph_save.json").exists()
    }

    pub fn is_struct_folder(path: &Path) -> bool {
        path.is_dir() && path.join("struct.json").exists()
    }

    pub fn is_enum_folder(path: &Path) -> bool {
        path.is_dir() && path.join("enum.json").exists()
    }

    pub fn is_trait_folder(path: &Path) -> bool {
        path.is_dir() && path.join("trait.json").exists()
    }

    pub fn is_alias_folder(path: &Path) -> bool {
        path.is_dir() && path.join("alias.json").exists()
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_string();

        let file_type = if path.is_dir() {
            if Self::is_class_folder(path) {
                FileType::Class
            } else if Self::is_struct_folder(path) {
                FileType::StructType
            } else if Self::is_enum_folder(path) {
                FileType::EnumType
            } else if Self::is_trait_folder(path) {
                FileType::TraitType
            } else if Self::is_alias_folder(path) {
                FileType::AliasType
            } else {
                FileType::Folder
            }
        } else {
            // Check for type system files first
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if file_name.ends_with(".struct.json") {
                FileType::StructType
            } else if file_name.ends_with(".enum.json") {
                FileType::EnumType
            } else if file_name.ends_with(".trait.json") {
                FileType::TraitType
            } else if file_name.ends_with(".alias.json") {
                FileType::AliasType
            } else if file_name.ends_with(".level.json") {
                FileType::LevelScene
            } else {
                match path.extension().and_then(|s| s.to_str()) {
                    Some("rs") => FileType::Script,
                    Some("pdaw") => FileType::DawProject,
                    Some("db") | Some("sqlite") | Some("sqlite3") => FileType::Database,
                    Some("toml") => FileType::Config,
                    Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("bmp") | Some("svg") => FileType::Image,
                    Some("wav") | Some("mp3") | Some("ogg") | Some("flac") => FileType::Audio,
                    Some("mp4") | Some("webm") | Some("avi") => FileType::Video,
                    Some("txt") | Some("md") | Some("pdf") => FileType::Document,
                    Some("zip") | Some("tar") | Some("gz") | Some("7z") => FileType::Archive,
                    _ => FileType::Other,
                }
            }
        };

        // Get file metadata
        let metadata = std::fs::metadata(path).ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata.and_then(|m| m.modified().ok());

        let is_folder = matches!(file_type, FileType::Folder);

        Some(FileItem {
            path: path.to_path_buf(),
            name,
            file_type,
            is_folder,
            size,
            modified,
        })
    }
}

// ============================================================================
// EVENTS - File manager events
// ============================================================================

#[derive(Clone, Debug)]
pub struct FileSelected {
    pub path: PathBuf,
    pub file_type: FileType,
}

#[derive(Clone, Debug)]
pub struct PopoutFileManagerEvent {
    pub project_path: Option<PathBuf>,
}
