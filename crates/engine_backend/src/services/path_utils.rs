//! Path Utilities for Services
//!
//! Shared utilities for path and URI operations used across backend services.

use std::path::Path;

/// Convert a file path to an LSP-compatible file:// URI
///
/// Handles platform-specific path separators and drive letters correctly.
///
/// # Arguments
/// * `path` - The file path to convert
///
/// # Returns
/// A file:// URI string suitable for LSP communication
///
/// # Examples
/// ```ignore
/// // Windows: C:\Users\file.rs -> file:///C:/Users/file.rs
/// // Unix: /home/user/file.rs -> file:///home/user/file.rs
/// let uri = path_to_uri(&PathBuf::from("C:\\Users\\file.rs"));
/// ```
pub fn path_to_uri(path: &Path) -> String {
    let path_str = path.to_string_lossy().replace("\\", "/");
    
    // Windows drive letter detection (C:/, c:/, D:/, etc.)
    if path_str.len() >= 2 && path_str.chars().nth(1) == Some(':') {
        format!("file:///{}", path_str)
    } else {
        format!("file://{}", path_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_windows_path() {
        let path = PathBuf::from("C:\\Users\\test\\file.rs");
        assert_eq!(path_to_uri(&path), "file:///C:/Users/test/file.rs");
    }

    #[test]
    fn test_unix_path() {
        let path = PathBuf::from("/home/user/file.rs");
        assert_eq!(path_to_uri(&path), "file:///home/user/file.rs");
    }
}
