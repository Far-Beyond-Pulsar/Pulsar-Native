//! # File I/O Module
//!
//! File system operations for the Pulsar visual programming system.
//!
//! This module provides comprehensive file system operations including:
//! - File operations (read, write, append, copy, move, delete)
//! - File metadata (size, permissions, modification time, type checks)
//! - Directory operations (create, remove, list, walk)
//! - Path manipulation (join, absolute, parent, filename, extension, stem)

use crate::blueprint;

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
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(content),
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
    match std::fs::write(path, content) {
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
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(mut file) => match file.write_all(content.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed to write to file: {}", e)),
        },
        Err(e) => Err(format!("Failed to open file for append: {}", e)),
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
    std::path::Path::new(&path).exists()
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
    match std::fs::remove_file(path) {
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
    match std::fs::copy(source, destination) {
        Ok(_) => Ok(()),
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
    match std::fs::rename(source, destination) {
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
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
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
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let permissions = metadata.permissions();
            Ok((
                !permissions.readonly(), // writable
                true,                    // readable (if we can get metadata, it's readable)
                false,                   // executable (platform dependent)
            ))
        }
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
    match std::fs::metadata(path) {
        Ok(metadata) => match metadata.modified() {
            Ok(time) => match time.duration_since(std::time::UNIX_EPOCH) {
                Ok(duration) => Ok(duration.as_secs()),
                Err(e) => Err(format!("Time error: {}", e)),
            },
            Err(e) => Err(format!("Failed to get modified time: {}", e)),
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
    std::path::Path::new(&path).is_file()
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
    std::path::Path::new(&path).is_dir()
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
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(content.lines().map(|s| s.to_string()).collect()),
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
    let content = lines.join("\n");
    match std::fs::write(path, content) {
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
    match std::fs::create_dir_all(path) {
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
    match std::fs::remove_dir_all(path) {
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
    std::path::Path::new(&path).is_dir()
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
    match std::fs::read_dir(path) {
        Ok(entries) => {
            let mut files = Vec::new();
            for entry in entries {
                if let Ok(entry) = entry {
                    if let Some(name) = entry.file_name().to_str() {
                        files.push(name.to_string());
                    }
                }
            }
            Ok(files)
        }
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
    use std::fs;

    fn walk_dir(dir: &str, files: &mut Vec<String>) -> Result<(), std::io::Error> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(path_str) = path.to_str() {
                    walk_dir(path_str, files)?;
                }
            } else if let Some(path_str) = path.to_str() {
                files.push(path_str.to_string());
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
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path.to_string_lossy().to_string()),
        Err(e) => Err(format!("Failed to get absolute path: {}", e)),
    }
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
