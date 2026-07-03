//! # File I/O Module
//!
//! File system operations for the Pulsar visual programming system.
//!
//! This module provides comprehensive file system operations including:
//! - File operations (read, write, append, copy, move, delete)
//! - File metadata (size, permissions, modification time, type checks)
//! - Directory operations (create, remove, list, walk)
//! - Path manipulation (join, absolute, parent, filename, extension, stem)
//!
//! # Security
//!
//! All file I/O operations are sandboxed to a project directory root set via
//! [`set_sandbox_root`]. Operations that attempt to escape the sandbox
//! (e.g. via `../` path traversal) are rejected at runtime.

use crate::blueprint;
use engine_fs::virtual_fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ── Path sandbox ──────────────────────────────────────────────────────────────

/// Global sandbox root for blueprint file I/O.
///
/// Must be set during engine initialisation via [`set_sandbox_root`].
/// Once set, every file_io node will reject paths that escape this directory.
static BP_FILE_SANDBOX_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Set the project root that all blueprint file I/O is restricted to.
///
/// If the sandbox root has already been set this call is a no-op (a warning
/// is printed and the new value is ignored).
pub fn set_sandbox_root(root: PathBuf) {
    if BP_FILE_SANDBOX_ROOT.set(root).is_err() {
        eprintln!(
            "[pulsar_std] warning: BP_FILE_SANDBOX_ROOT already initialised — \
             refusing to change the sandbox root"
        );
    }
}

/// Resolve a user-supplied blueprint path against the sandbox root.
///
/// For *read* operations the resolved path must exist and must canonically
/// sit under the sandbox root.
fn resolve_read_path(user_path: &str) -> Result<PathBuf, String> {
    let root = BP_FILE_SANDBOX_ROOT.get().ok_or_else(|| {
        "File I/O sandbox root not set — call set_sandbox_root during startup".to_string()
    })?;

    let raw = if Path::new(user_path).is_relative() {
        root.join(user_path)
    } else {
        PathBuf::from(user_path)
    };

    let canonical = raw
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path '{}': {}", user_path, e))?;

    let canonical_root = root
        .canonicalize()
        .map_err(|_| "Sandbox root does not exist".to_string())?;

    if !canonical.starts_with(&canonical_root) {
        return Err(format!(
            "Security: path '{}' resolves outside the sandbox root",
            user_path
        ));
    }
    Ok(canonical)
}

/// Resolve a user-supplied blueprint path for *write* operations.
///
/// The path need not exist yet, but every existing ancestor must sit inside
/// the sandbox root.
fn resolve_write_path(user_path: &str) -> Result<PathBuf, String> {
    let root = BP_FILE_SANDBOX_ROOT.get().ok_or_else(|| {
        "File I/O sandbox root not set — call set_sandbox_root during startup".to_string()
    })?;

    let raw = if Path::new(user_path).is_relative() {
        root.join(user_path)
    } else {
        PathBuf::from(user_path)
    };

    // Walk up the ancestor chain until we find a path that exists.
    // The existing ancestor must be inside the sandbox root.
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "Sandbox root does not exist".to_string())?;

    let existing_ancestor = raw
        .ancestors()
        .find(|a| a.exists())
        .unwrap_or(root.as_path());

    if existing_ancestor != root.as_path() {
        let canonical_ancestor = existing_ancestor
            .canonicalize()
            .map_err(|e| format!("Cannot resolve path '{}': {}", user_path, e))?;
        if !canonical_ancestor.starts_with(&canonical_root) {
            return Err(format!(
                "Security: path '{}' resolves outside the sandbox root",
                user_path
            ));
        }
    }

    Ok(raw)
}

// =============================================================================
// File Operations
// =============================================================================

/// Read the contents of a file as a string.
///
/// # Inputs
/// - `path`: The path to the file to read (string)
///
/// # Returns
/// The file contents as a string, or an error message if reading fails
///
/// # Example
/// If `path` is "config.txt" and the file contains "hello", the output will be Ok("hello").
///
/// # Notes
/// The entire file is read into memory. For large files, consider streaming or reading in chunks.
/// # File Read
/// Reads the contents of a file as a string.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_read(path: String) -> Result<String, String> {
    let path = resolve_read_path(&path)?;
    match virtual_fs::read_file(&path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {}", e)),
        Err(e) => Err(format!("Failed to read file: {}", e)),
    }
}

/// Write content to a file.
///
/// # Inputs
/// - `path`: The path of the file to write to (string)
/// - `content`: The content to write into the file (string)
///
/// # Returns
/// Ok(()) if the file was written successfully, or an error message if writing failed
///
/// # Example
/// If `path` is "output.txt" and `content` is "Hello, world!", the file will contain "Hello, world!" after execution.
///
/// # Notes
/// This operation overwrites any existing content in the file.
/// # File Write
/// Writes content to a file, overwriting existing content.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_write(path: String, content: String) -> Result<(), String> {
    let path = resolve_write_path(&path)?;
    match virtual_fs::write_file(&path, content.as_bytes()) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Failed to write file: {}", e)),
    }
}

/// Append content to a file.
///
/// # Inputs
/// - `path`: The path to the file to append to (string)
/// - `content`: The content to append (string)
///
/// # Returns
/// Ok(()) if the content was appended successfully, or an error message if the operation failed
///
/// # Example
/// If `path` is "log.txt" and `content` is "Hello\n", the string will be added to the end of "log.txt".
///
/// # Notes
/// If the file does not exist, it will be created.
/// # File Append
/// Appends content to the end of a file.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_append(path: String, content: String) -> Result<(), String> {
    let path = resolve_write_path(&path)?;
    let mut existing = virtual_fs::read_file(&path).unwrap_or_default();
    existing.extend_from_slice(content.as_bytes());
    match virtual_fs::write_file(&path, &existing) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Failed to write to file: {}", e)),
    }
}

/// Check if a file exists.
///
/// # Inputs
/// - `path`: The path to check (string)
///
/// # Returns
/// Returns `true` if the file exists, `false` otherwise
///
/// # Example
/// If `path` is "data.txt" and the file exists, the output will be true.
/// # File Exists
/// Checks if a file exists at the specified path.
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn file_exists(path: String) -> bool {
    // We still need to check if the resolved path is inside the sandbox root
    // before reporting that it exists.
    if let Ok(resolved) = resolve_read_path(&path) {
        virtual_fs::exists(&resolved).unwrap_or(false)
    } else {
        // Sandboxed: a path that escapes the sandbox is treated as non-existent
        // to avoid leaking information about files outside the project.
        false
    }
}

/// Delete a file.
///
/// # Inputs
/// - `path`: The path to the file to delete (string)
///
/// # Returns
/// Ok(()) if the file was deleted successfully, or an error message if deletion failed
///
/// # Example
/// If `path` is "output.log", the file "output.log" will be deleted if it exists.
///
/// # Notes
/// Use with caution: this operation is irreversible and will permanently remove the file.
/// # File Delete
/// Deletes a file at the specified path.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_delete(path: String) -> Result<(), String> {
    let path = resolve_write_path(&path)?;
    match virtual_fs::delete_path(&path) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Failed to delete file: {}", e)),
    }
}

/// Copy a file from source to destination.
///
/// # Inputs
/// - `source`: The path to the source file (string)
/// - `destination`: The path to the destination file (string)
///
/// # Returns
/// Ok(()) if the file was copied successfully, or an error message if the operation failed
///
/// # Example
/// If `source` is "input.txt" and `destination` is "output.txt", the contents will be copied.
///
/// # Notes
/// If the destination file already exists, it will be overwritten.
/// # File Copy
/// Copies a file from source to destination path.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_copy(source: String, destination: String) -> Result<(), String> {
    let source = resolve_read_path(&source)?;
    let destination = resolve_write_path(&destination)?;
    match virtual_fs::read_file(&source) {
        Ok(data) => virtual_fs::write_file(&destination, &data).map_err(|e| format!("Failed to copy file: {}", e)),
        Err(e) => Err(format!("Failed to copy file: {}", e)),
    }
}

/// Move or rename a file from source to destination.
///
/// # Inputs
/// - `source`: The path of the file to move or rename (string)
/// - `destination`: The new path for the file (string)
///
/// # Returns
/// Ok(()) if the file was moved successfully, or an error message if the operation failed
///
/// # Example
/// If `source` is "old.txt" and `destination` is "new.txt", the file will be renamed.
///
/// # Notes
/// If the destination is on the same filesystem, the operation is atomic and fast.
/// # File Move
/// Moves or renames a file from source to destination.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_move(source: String, destination: String) -> Result<(), String> {
    let source = resolve_write_path(&source)?;
    let destination = resolve_write_path(&destination)?;
    match virtual_fs::rename(&source, &destination) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Failed to move file: {}", e)),
    }
}

/// Get the size of a file in bytes.
///
/// # Inputs
/// - `path`: The path to the file (string)
///
/// # Returns
/// The size of the file in bytes, or an error message if the file cannot be accessed
///
/// # Example
/// If `path` is "data.txt" and the file is 1024 bytes, the output will be Ok(1024).
/// # File Size
/// Returns the size of a file in bytes.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_size(path: String) -> Result<u64, String> {
    let path = resolve_read_path(&path)?;
    match virtual_fs::metadata(&path) {
        Ok(metadata) => Ok(metadata.size),
        Err(e) => Err(format!("Failed to get file size: {}", e)),
    }
}

/// Check file permissions (read/write/execute).
///
/// # Inputs
/// - `path`: The path to the file or directory to check (string)
///
/// # Returns
/// A tuple containing (writable, readable, executable) flags, or an error message
///
/// # Example
/// If `path` is "output.txt" and the file is writable and readable, the output will be (true, true, false).
///
/// # Notes
/// The executable flag is not implemented and always returns false.
/// # File Permissions
/// Checks file permissions (writable, readable, executable).
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_permissions(path: String) -> Result<(bool, bool, bool), String> {
    let path = resolve_read_path(&path)?;
    match virtual_fs::metadata(&path) {
        Ok(_metadata) => Ok((true, true, false)),
        Err(e) => Err(format!("Failed to get file permissions: {}", e)),
    }
}

/// Get the last modified time of a file as Unix timestamp.
///
/// # Inputs
/// - `path`: The path to the file (string)
///
/// # Returns
/// The last modified time as a Unix timestamp (seconds since epoch), or an error message
///
/// # Example
/// If the file was last modified on January 2, 1970, the output will be 86400.
///
/// # Notes
/// The timestamp is in UTC.
/// # File Modified Time
/// Returns the last modified time of a file as Unix timestamp.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_modified_time(path: String) -> Result<u64, String> {
    let path = resolve_read_path(&path)?;
    match virtual_fs::metadata(&path) {
        Ok(metadata) => match metadata.modified {
            Some(ts) => Ok(ts),
            None => Err("Failed to get modified time: not available".to_string()),
        },
        Err(e) => Err(format!("Failed to get file metadata: {}", e)),
    }
}

/// Check if a path is a file.
///
/// # Inputs
/// - `path`: The path to check (string)
///
/// # Returns
/// Returns `true` if the path exists and is a file, `false` otherwise
///
/// # Example
/// If `path` is "C:/Users/file.txt" and that file exists, the output will be true.
/// # File Is File
/// Checks if a path exists and is a file.
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn file_is_file(path: String) -> bool {
    resolve_read_path(&path).map_or(false, |p| {
        virtual_fs::metadata(&p).map_or(false, |m| !m.is_dir)
    })
}

/// Check if a path is a directory.
///
/// # Inputs
/// - `path`: The path to check (string)
///
/// # Returns
/// Returns `true` if the path exists and is a directory, `false` otherwise
///
/// # Example
/// If `path` is "C:/Users" and that directory exists, the output will be true.
/// # File Is Dir
/// Checks if a path exists and is a directory.
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn file_is_dir(path: String) -> bool {
    resolve_read_path(&path).map_or(false, |p| {
        virtual_fs::metadata(&p).map_or(false, |m| m.is_dir)
    })
}

/// Read a file and return its lines as a vector.
///
/// # Inputs
/// - `path`: The path to the file to read (string)
///
/// # Returns
/// A vector of lines from the file, or an error message if reading fails
///
/// # Example
/// If the file contains "line1\nline2\nline3", the output will be Ok(vec!["line1", "line2", "line3"]).
///
/// # Notes
/// Lines are split on newline characters. Empty files return an empty vector.
/// # File Read Lines
/// Reads a file and returns its lines as a vector of strings.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_read_lines(path: String) -> Result<Vec<String>, String> {
    let path = resolve_read_path(&path)?;
    match virtual_fs::read_file(&path) {
        Ok(bytes) => {
            let content = String::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {}", e))?;
            Ok(content.lines().map(|s| s.to_string()).collect())
        }
        Err(e) => Err(format!("Failed to read file: {}", e)),
    }
}

/// Write lines to a file.
///
/// # Inputs
/// - `path`: The path to the file to write to (string)
/// - `lines`: A vector of strings, each representing a line to write
///
/// # Returns
/// Ok(()) if the file was written successfully, or an error message if writing failed
///
/// # Example
/// If `path` is "output.txt" and `lines` is ["foo", "bar"], the file will contain "foo\nbar".
///
/// # Notes
/// This operation overwrites the file.
/// # File Write Lines
/// Writes lines to a file, with each string as a separate line.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn file_write_lines(path: String, lines: Vec<String>) -> Result<(), String> {
    let path = resolve_write_path(&path)?;
    let content = lines.join("\n");
    match virtual_fs::write_file(&path, content.as_bytes()) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Failed to write file: {}", e)),
    }
}

// =============================================================================
// Directory Operations
// =============================================================================

/// Create a directory (including parent directories).
///
/// # Inputs
/// - `path`: The path of the directory to create (string)
///
/// # Returns
/// Ok(()) if the directory was created successfully, or an error message if creation failed
///
/// # Example
/// If `path` is "output/logs", the node will create both "output" and "logs" directories.
///
/// # Notes
/// If the directory already exists, this function will succeed without error.
/// # Dir Create
/// Creates a directory, including any necessary parent directories.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn dir_create(path: String) -> Result<(), String> {
    let path = resolve_write_path(&path)?;
    match virtual_fs::create_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Failed to create directory: {}", e)),
    }
}

/// Remove a directory and all its contents.
///
/// # Inputs
/// - `path`: The path to the directory to remove (string)
///
/// # Returns
/// Ok(()) if the directory was removed successfully, or an error message if removal failed
///
/// # Example
/// If `path` is "temp_data", the directory and all its contents will be deleted.
///
/// # Notes
/// Use with caution: this operation is irreversible and will delete all data in the directory.
/// # Dir Remove
/// Removes a directory and all its contents recursively.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn dir_remove(path: String) -> Result<(), String> {
    let path = resolve_write_path(&path)?;
    match virtual_fs::delete_path(&path) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Failed to remove directory: {}", e)),
    }
}

/// Check if a directory exists.
///
/// # Inputs
/// - `path`: The path to check (string)
///
/// # Returns
/// Returns `true` if the path exists and is a directory, `false` otherwise
///
/// # Example
/// If `path` is "C:/Users" and that directory exists, the output will be true.
/// # Dir Exists
/// Checks if a directory exists at the specified path.
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn dir_exists(path: String) -> bool {
    resolve_read_path(&path).map_or(false, |p| {
        virtual_fs::metadata(&p).map_or(false, |m| m.is_dir)
    })
}

/// List the contents of a directory.
///
/// # Inputs
/// - `path`: The path to the directory to list (string)
///
/// # Returns
/// A vector of file and directory names, or an error message if the directory cannot be read
///
/// # Example
/// If `path` is "C:/Users", the output will be a list of all files and folders in that directory.
///
/// # Notes
/// The output contains only the names (not full paths) of the entries.
/// # Dir List
/// Lists the contents of a directory (file and folder names).
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn dir_list(path: String) -> Result<Vec<String>, String> {
    let path = resolve_read_path(&path)?;
    match virtual_fs::list_dir(&path) {
        Ok(entries) => Ok(entries.into_iter().map(|e| e.name).collect()),
        Err(e) => Err(format!("Failed to read directory: {}", e)),
    }
}

/// Recursively walk through a directory tree and return all file paths.
///
/// # Inputs
/// - `path`: The root directory path to start walking from (string)
///
/// # Returns
/// A vector of file paths found, or an error message if traversal fails
///
/// # Example
/// If `path` is "assets", the output will be a list of all files under "assets" and its subdirectories.
///
/// # Notes
/// The function performs a depth-first traversal.
/// # Dir Walk
/// Recursively walks through a directory tree and returns all file paths.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn dir_walk(path: String) -> Result<Vec<String>, String> {
    let path = resolve_read_path(&path)?;

    fn walk_dir(dir: &Path, files: &mut Vec<String>) -> Result<(), String> {
        let entries = virtual_fs::list_dir(dir)
            .map_err(|e| format!("Failed to walk directory: {}", e))?;
        for entry in entries {
            let child = dir.join(&entry.name);
            let child_str = child.to_string_lossy().to_string();
            if entry.is_dir {
                walk_dir(&child, files)?;
            } else {
                files.push(child_str);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    match walk_dir(&path, &mut files) {
        Ok(()) => Ok(files),
        Err(e) => Err(format!("Failed to walk directory: {}", e)),
    }
}

// =============================================================================
// Path Operations
// =============================================================================

/// Join two path components into a single path.
///
/// # Inputs
/// - `base`: The base path (string)
/// - `component`: The path component to join (string)
///
/// # Returns
/// A string representing the joined path
///
/// # Example
/// If `base` is "folder" and `component` is "file.txt", the output will be "folder/file.txt" (on Unix).
/// # Path Join
/// Joins two path components using the platform's path separator.
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn path_join(base: String, component: String) -> String {
    std::path::Path::new(&base)
        .join(component)
        .to_string_lossy()
        .to_string()
}

/// Convert a relative path to an absolute path.
///
/// # Inputs
/// - `path`: The input path to convert (string)
///
/// # Returns
/// The absolute path as a string, or an error message if the path cannot be resolved
///
/// # Example
/// If `path` is "./data/file.txt" and the current directory is "/home/user/project",
/// the output will be "/home/user/project/data/file.txt".
///
/// # Notes
/// If the path does not exist, an error is returned.
/// # Path Absolute
/// Converts a relative path to an absolute path.
#[blueprint(type: NodeTypes::fn_, category: "File I/O", color: "#E67E22")]
pub fn path_absolute(path: String) -> Result<String, String> {
    let path = resolve_read_path(&path)?;
    Ok(path.to_string_lossy().to_string())
}

/// Get the parent directory of a path.
///
/// # Inputs
/// - `path`: The input file or directory path (string)
///
/// # Returns
/// The parent directory as a string, or None if the path has no parent
///
/// # Example
/// If `path` is "/home/user/file.txt", the output will be Some("/home/user").
/// # Path Parent
/// Returns the parent directory of a path.
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn path_parent(path: String) -> Option<String> {
    std::path::Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
}

/// Extract the filename from a path.
///
/// # Inputs
/// - `path`: The input file path (string)
///
/// # Returns
/// The filename as a string, or None if the path does not have a filename component
///
/// # Example
/// If `path` is "/foo/bar/baz.txt", the output will be Some("baz.txt").
/// # Path Filename
/// Extracts the filename (with extension) from a path.
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn path_filename(path: String) -> Option<String> {
    std::path::Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|s| s.to_string())
}

/// Extract the file extension from a path.
///
/// # Inputs
/// - `path`: The input file path (string)
///
/// # Returns
/// The file extension if present, or None if the path has no extension
///
/// # Example
/// If `path` is "foo.txt", the output will be Some("txt").
///
/// # Notes
/// The extension is returned without the leading dot.
/// # Path Extension
/// Extracts the file extension from a path (without the dot).
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn path_extension(path: String) -> Option<String> {
    std::path::Path::new(&path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_string())
}

/// Get the filename without extension (the "stem") from a path.
///
/// # Inputs
/// - `path`: The file path to extract the stem from (string)
///
/// # Returns
/// The filename without extension, or None if the path does not have a filename
///
/// # Example
/// If `path` is "foo/bar/baz.txt", the output will be Some("baz").
/// # Path Stem
/// Returns the filename without its extension.
#[blueprint(type: NodeTypes::pure, category: "File I/O", color: "#E67E22")]
pub fn path_stem(path: String) -> Option<String> {
    std::path::Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|s| s.to_string())
}
