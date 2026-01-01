//! Simple file synchronization using hash-based diffing
//!
//! Architecture:
//! 1. Host creates manifest: { path -> hash } for all project files
//! 2. Joiner compares with local files to find differences
//! 3. Joiner requests missing/changed files
//! 4. Host sends file contents
//! 5. Joiner writes files to disk

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// File manifest entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

/// Complete file manifest for a project
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub files: Vec<FileEntry>,
}

/// Diff between two manifests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDiff {
    pub files_to_add: Vec<String>,      // New files
    pub files_to_update: Vec<String>,   // Modified files
    pub files_to_delete: Vec<String>,   // Deleted files
}

impl SyncDiff {
    pub fn has_changes(&self) -> bool {
        !self.files_to_add.is_empty()
            || !self.files_to_update.is_empty()
            || !self.files_to_delete.is_empty()
    }

    pub fn change_count(&self) -> usize {
        self.files_to_add.len() + self.files_to_update.len() + self.files_to_delete.len()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} to add, {} to update, {} to delete",
            self.files_to_add.len(),
            self.files_to_update.len(),
            self.files_to_delete.len()
        )
    }
}

/// Default ignore patterns
const DEFAULT_IGNORES: &[&str] = &[
    ".git",
    "target",
    "dist",
    "build",
    "node_modules",
    ".vscode",
    ".idea",
    ".vs",
    "*.exe",
    "*.dll",
    "*.so",
    "*.dylib",
    "*.log",
    ".DS_Store",
    "Thumbs.db",
];

/// Check if a path should be ignored
fn should_ignore(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    for pattern in DEFAULT_IGNORES {
        if pattern.contains('*') {
            // Simple glob matching for extensions
            if pattern.starts_with("*.") {
                let ext = &pattern[2..];
                if path_str.ends_with(ext) {
                    return true;
                }
            }
        } else {
            // Exact component matching
            if path.components().any(|c| c.as_os_str() == *pattern) {
                return true;
            }
        }
    }

    false
}

/// Calculate SHA256 hash of file contents
fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let data = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Create file manifest for a project directory
pub fn create_manifest(project_root: &Path) -> Result<FileManifest, std::io::Error> {
    tracing::debug!("SIMPLE_SYNC: Creating manifest for {:?}", project_root);
    let mut files = Vec::new();

    for entry in WalkDir::new(project_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_ignore(e.path()))
    {
        let entry = entry?;

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let relative_path = path.strip_prefix(project_root)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        match hash_file(path) {
            Ok(hash) => {
                let metadata = fs::metadata(path)?;
                // Normalize path separators to forward slashes for cross-platform compatibility
                let normalized_path = relative_path
                    .to_string_lossy()
                    .replace('\\', "/");
                
                tracing::debug!("SIMPLE_SYNC: Added file: {} (hash: {}, size: {})", 
                    normalized_path, &hash[..8], metadata.len());
                
                files.push(FileEntry {
                    path: normalized_path,
                    hash,
                    size: metadata.len(),
                });
            }
            Err(e) => {
                tracing::warn!("SIMPLE_SYNC: Failed to hash {:?}: {}", path, e);
            }
        }
    }

    tracing::debug!("SIMPLE_SYNC: Created manifest with {} files", files.len());
    if files.len() > 0 {
        tracing::debug!("SIMPLE_SYNC: Sample files: {:?}", 
            files.iter().take(3).map(|f| &f.path).collect::<Vec<_>>());
    }
    Ok(FileManifest { files })
}

/// Compare remote manifest with local files to compute diff
pub fn compute_diff(
    project_root: &Path,
    remote_manifest: &FileManifest,
) -> Result<SyncDiff, std::io::Error> {
    tracing::debug!("SIMPLE_SYNC: Computing diff against remote manifest ({} files)", remote_manifest.files.len());
    
    if remote_manifest.files.len() > 0 {
        tracing::debug!("SIMPLE_SYNC: Remote sample files: {:?}", 
            remote_manifest.files.iter().take(3).map(|f| &f.path).collect::<Vec<_>>());
    }

    // Build local manifest
    let local_manifest = create_manifest(project_root)?;
    tracing::debug!("SIMPLE_SYNC: Local manifest has {} files", local_manifest.files.len());

    // Build lookup maps
    let mut local_map: HashMap<String, String> = local_manifest
        .files
        .iter()
        .map(|f| (f.path.clone(), f.hash.clone()))
        .collect();

    let remote_map: HashMap<String, String> = remote_manifest
        .files
        .iter()
        .map(|f| (f.path.clone(), f.hash.clone()))
        .collect();

    let mut files_to_add = Vec::new();
    let mut files_to_update = Vec::new();

    // Find new and modified files
    for file in &remote_manifest.files {
        match local_map.get(&file.path) {
            None => {
                // File doesn't exist locally
                tracing::debug!("SIMPLE_SYNC: File to ADD: {}", file.path);
                files_to_add.push(file.path.clone());
            }
            Some(local_hash) => {
                // File exists - check if different
                if local_hash != &file.hash {
                    tracing::debug!("SIMPLE_SYNC: File to UPDATE: {} (local: {}, remote: {})", 
                        file.path, &local_hash[..8], &file.hash[..8]);
                    files_to_update.push(file.path.clone());
                }
                // Remove from local map (used to find deletions)
                local_map.remove(&file.path);
            }
        }
    }

    // Remaining files in local_map are deleted in remote
    let files_to_delete: Vec<String> = local_map.keys().cloned().collect();
    
    if !files_to_delete.is_empty() {
        tracing::debug!("SIMPLE_SYNC: Files to DELETE: {:?}", &files_to_delete);
    }

    let diff = SyncDiff {
        files_to_add,
        files_to_update,
        files_to_delete,
    };

    tracing::debug!("SIMPLE_SYNC: Diff computed: {}", diff.summary());
    Ok(diff)
}

/// Apply file changes from received file data
pub fn apply_files(
    project_root: &Path,
    files: Vec<(String, Vec<u8>)>,
) -> Result<usize, std::io::Error> {
    tracing::debug!("SIMPLE_SYNC: Applying {} files to {:?}", files.len(), project_root);
    let mut written_count = 0;

    for (relative_path, data) in files {
        let full_path = project_root.join(&relative_path);

        // Create parent directories
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write file
        fs::write(&full_path, &data)?;
        tracing::debug!("SIMPLE_SYNC: Wrote {:?} ({} bytes)", full_path, data.len());
        written_count += 1;
    }

    tracing::debug!("SIMPLE_SYNC: Successfully wrote {} files", written_count);
    Ok(written_count)
}

/// Delete files that were removed
pub fn delete_files(
    project_root: &Path,
    file_paths: Vec<String>,
) -> Result<usize, std::io::Error> {
    tracing::debug!("SIMPLE_SYNC: Deleting {} files from {:?}", file_paths.len(), project_root);
    let mut deleted_count = 0;

    for relative_path in file_paths {
        let full_path = project_root.join(&relative_path);

        if full_path.exists() {
            fs::remove_file(&full_path)?;
            tracing::debug!("SIMPLE_SYNC: Deleted {:?}", full_path);
            deleted_count += 1;
        }
    }

    tracing::debug!("SIMPLE_SYNC: Successfully deleted {} files", deleted_count);
    Ok(deleted_count)
}

/// Read file contents for transfer
pub fn read_files(
    project_root: &Path,
    file_paths: Vec<String>,
) -> Result<Vec<(String, Vec<u8>)>, std::io::Error> {
    tracing::debug!("SIMPLE_SYNC: Reading {} files from {:?}", file_paths.len(), project_root);
    let mut files = Vec::new();

    for relative_path in file_paths {
        let full_path = project_root.join(&relative_path);
        let data = fs::read(&full_path)?;
        let data_len = data.len();
        files.push((relative_path, data));
        tracing::debug!("SIMPLE_SYNC: Read {:?} ({} bytes)", full_path, data_len);
    }

    tracing::debug!("SIMPLE_SYNC: Successfully read {} files", files.len());
    Ok(files)
}
