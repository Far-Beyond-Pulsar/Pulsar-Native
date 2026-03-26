//! Path utility functions for cloud and local paths
//!
//! Provides helper functions for working with cloud paths and path normalization.

use std::path::Path;

/// Return `true` when `path` carries the `cloud+pulsar://` scheme, indicating
/// it refers to a file on a remote `pulsar-host` server rather than on disk.
///
/// Normalizes Windows backslashes to forward slashes before checking so that
/// paths stored in a `PathBuf` on Windows still match the URI scheme prefix.
pub fn is_cloud_path(path: &Path) -> bool {
    path.to_string_lossy()
        .replace('\\', "/")
        .starts_with("cloud+pulsar://")
}

/// Normalize a path by converting backslashes to forward slashes.
///
/// This is useful for ensuring consistent path handling across platforms,
/// especially when working with cloud paths or URI-like paths.
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Join path components using forward slashes, suitable for cloud paths.
///
/// Unlike `PathBuf::join()` which uses platform-specific separators,
/// this always uses forward slashes.
pub fn cloud_join(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if base.is_empty() {
        path.to_string()
    } else if path.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", base, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_cloud_path() {
        assert!(is_cloud_path(Path::new("cloud+pulsar://host/project/file.txt")));
        assert!(!is_cloud_path(Path::new("/local/path/file.txt")));
        assert!(!is_cloud_path(Path::new("C:\\local\\file.txt")));
    }

    #[test]
    fn test_cloud_join() {
        assert_eq!(cloud_join("cloud+pulsar://host/proj", "subdir/file.txt"),
                   "cloud+pulsar://host/proj/subdir/file.txt");
        assert_eq!(cloud_join("cloud+pulsar://host/proj/", "/subdir/file.txt"),
                   "cloud+pulsar://host/proj/subdir/file.txt");
        assert_eq!(cloud_join("", "file.txt"), "file.txt");
        assert_eq!(cloud_join("base", ""), "base");
    }
}
