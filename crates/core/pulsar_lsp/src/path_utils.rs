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

// The filesystem path a `file://` URI names: parsed in `pulsar_language`,
// which language plugins use without the editor.
pub use pulsar_language::lsp_edits::uri_to_path;

/// A string that is equal for every spelling of the same file: separators are
/// forward slashes, and on Windows the case is folded (drive letters arrive as
/// `C:` from the editor but `c:` from the server).
pub fn path_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

/// [`path_key`] of the file a URI names.
pub fn uri_key(uri: &str) -> String {
    path_key(&uri_to_path(uri))
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

    #[test]
    fn uri_decodes_to_a_path() {
        assert_eq!(
            uri_to_path("file:///c%3A/Users/a%20b/file.rs"),
            PathBuf::from("c:/Users/a b/file.rs")
        );
        assert_eq!(
            uri_to_path("file:///home/user/file.rs"),
            PathBuf::from("/home/user/file.rs")
        );
        assert_eq!(uri_to_path("file:///C:/x%"), PathBuf::from("C:/x%"));
    }

    #[test]
    fn every_spelling_of_a_file_shares_a_key() {
        #[cfg(windows)]
        assert_eq!(
            path_key(Path::new("C:\\Users\\Test\\file.rs")),
            uri_key("file:///c%3A/Users/Test/file.rs")
        );
        assert_eq!(
            uri_key("file:///home/a/b.rs"),
            path_key(Path::new("/home/a/b.rs"))
        );
    }
}
