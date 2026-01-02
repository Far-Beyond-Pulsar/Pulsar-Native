/// Utility module for file scanning and categorization
///
/// This module provides functionality for discovering and categorizing files
/// in a project directory for display in file browsers and command palettes.
///
/// IMPORTANT: File type detection here is ONLY for display purposes (icons, labels).
/// The plugin system's registry determines what files can actually be opened.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum FileType {
    Folder,
    Class,  // A folder containing graph_save.json  
    File,   // Any other file - plugin system will determine if openable
}

impl FileType {
    /// Get a display name for the file type
    pub fn display_name(&self) -> &'static str {
        match self {
            FileType::Folder => "Folder",
            FileType::Class => "Blueprint Class",
            FileType::File => "File",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileInfo {
    pub path: PathBuf,
    pub name: String,
    pub file_type: FileType,
}

impl FileInfo {
    /// Check if a path is a blueprint class folder (contains graph_save.json)
    pub fn is_class_folder(path: &Path) -> bool {
        path.is_dir() && path.join("graph_save.json").exists()
    }

    /// Detect file type from path (for display purposes only)
    pub fn detect_file_type(path: &Path) -> FileType {
        if path.is_dir() {
            if Self::is_class_folder(path) {
                FileType::Class
            } else {
                FileType::Folder
            }
        } else {
            FileType::File
        }
    }

    /// Create FileInfo from a path
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_string();
        let file_type = Self::detect_file_type(path);

        Some(FileInfo {
            path: path.to_path_buf(),
            name,
            file_type,
        })
    }

    /// Get a display string for this file (includes type and path)
    pub fn display_string(&self) -> String {
        format!("{} - {}", self.name, self.file_type.display_name())
    }
}

/// Recursively find all files in a directory (plugin system determines if openable)
pub fn find_openable_files(root: &Path, max_depth: Option<usize>) -> Vec<FileInfo> {
    let mut results = Vec::new();
    find_files_recursive(root, root, &mut results, 0, max_depth.unwrap_or(10));
    results
}

fn find_files_recursive(
    root: &Path,
    current: &Path,
    results: &mut Vec<FileInfo>,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }

    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip hidden files and common ignore patterns
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
        }

        if let Some(file_info) = FileInfo::from_path(&path) {
            let is_class = file_info.file_type == FileType::Class;
            let is_file = file_info.file_type == FileType::File;

            // Add all non-folder items (classes and files)
            if is_class || is_file {
                results.push(file_info);
            }

            // Recurse into directories (but not class folders, as they're treated as files)
            if path.is_dir() && !is_class {
                find_files_recursive(root, &path, results, depth + 1, max_depth);
            }
        }
    }
}

/// Search for files matching a query string
pub fn search_files(files: &[FileInfo], query: &str) -> Vec<FileInfo> {
    if query.is_empty() {
        return files.to_vec();
    }

    let query_lower = query.to_lowercase();
    let mut matches: Vec<(usize, FileInfo)> = files
        .iter()
        .filter_map(|file| {
            let name_lower = file.name.to_lowercase();
            let path_str = file.path.to_string_lossy().to_lowercase();

            // Calculate match score (lower is better)
            let score = if name_lower == query_lower {
                0 // Exact match
            } else if name_lower.starts_with(&query_lower) {
                1 // Starts with query
            } else if name_lower.contains(&query_lower) {
                2 // Contains in name
            } else if path_str.contains(&query_lower) {
                3 // Contains in path
            } else {
                return None;
            };

            Some((score, file.clone()))
        })
        .collect();

    // Sort by score, then by name
    matches.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name))
    });

    matches.into_iter().map(|(_, file)| file).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_type_detection() {
        // File type detection is simplified - just File, Folder, or Class
        let script = PathBuf::from("test.rs");
        assert_eq!(FileInfo::detect_file_type(&script), FileType::File);

        let daw = PathBuf::from("project.pdaw");
        assert_eq!(FileInfo::detect_file_type(&daw), FileType::File);

        let config = PathBuf::from("config.toml");
        assert_eq!(FileInfo::detect_file_type(&config), FileType::File);
    }

    #[test]
    fn test_file_search() {
        let files = vec![
            FileInfo {
                path: PathBuf::from("/project/src/main.rs"),
                name: "main.rs".to_string(),
                file_type: FileType::File,
            },
            FileInfo {
                path: PathBuf::from("/project/src/utils.rs"),
                name: "utils.rs".to_string(),
                file_type: FileType::File,
            },
            FileInfo {
                path: PathBuf::from("/project/audio/song.pdaw"),
                name: "song.pdaw".to_string(),
                file_type: FileType::File,
            },
        ];

        let results = search_files(&files, "main");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "main.rs");

        let results = search_files(&files, "rs");
        assert_eq!(results.len(), 2);

        let results = search_files(&files, "");
        assert_eq!(results.len(), 3);
    }
}
