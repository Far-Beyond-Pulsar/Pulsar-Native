//! Path and URI helpers for LSP communication.

use std::path::Path;

/// Convert a file path to an LSP-compatible `file://` URI.
///
/// Handles Windows drive letters (C:\…) and Unix paths (/home/…) correctly.
pub fn path_to_uri(path: &Path) -> String {
    let path_str = path.to_string_lossy().replace('\\', "/");
    // Windows drive-letter detection (C:/, D:/, …)
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
