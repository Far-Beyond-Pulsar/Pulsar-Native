//! Path and URI helpers for LSP communication.

use std::path::{Path, PathBuf};

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

/// Decodes `%XX` escapes (rust-analyzer sends `file:///c%3A/Users/…`).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                decoded.push(value);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The filesystem path a `file://` URI names.
pub fn uri_to_path(uri: &str) -> PathBuf {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let decoded = percent_decode(rest);
    // `/C:/Users/…` is a Windows drive path with a leading slash.
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' && bytes[1].is_ascii_alphabetic() {
        PathBuf::from(&decoded[1..])
    } else {
        PathBuf::from(decoded)
    }
}

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

/// Whether `file` lies inside `folder` (both compared by [`path_key`]).
pub fn is_within(file: &Path, folder: &Path) -> bool {
    let file = path_key(file);
    let folder = path_key(folder);
    let folder = folder.trim_end_matches('/');
    file == folder || file.starts_with(&format!("{folder}/"))
}

/// The workspace folder the language server needs for `file`, or `None` when
/// it already covers it.
///
/// rust-analyzer only knows the folders it was given. A file outside all of
/// them is an unattached file: it has no crate graph, so even standard-library
/// items answer nothing. Editors like VS Code register the file's own Cargo
/// workspace with the running server; this finds the folder to register: the
/// outermost directory above `file` holding a `Cargo.toml`.
///
/// Toolchain and dependency sources (`rustlib`, the cargo registry, git
/// checkouts) are never folders: the server reaches them through whichever
/// project depends on them.
pub fn workspace_folder_for(file: &Path, existing: &[PathBuf]) -> Option<PathBuf> {
    if existing.iter().any(|folder| is_within(file, folder)) {
        return None;
    }
    let key = path_key(file);
    if key.contains("/rustlib/")
        || key.contains("/.cargo/registry/")
        || key.contains("/.cargo/git/")
    {
        return None;
    }
    file.ancestors()
        .skip(1)
        .filter(|dir| dir.join("Cargo.toml").is_file())
        .last()
        .map(Path::to_path_buf)
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

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulsar_lsp_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_outside_every_folder_gets_the_outermost_cargo_workspace() {
        let root = scratch("outer");
        std::fs::create_dir_all(root.join("crates/inner/src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]").unwrap();
        std::fs::write(root.join("crates/inner/Cargo.toml"), "[package]").unwrap();
        let file = root.join("crates/inner/src/lib.rs");
        std::fs::write(&file, "").unwrap();

        let project = scratch("project");
        assert_eq!(
            workspace_folder_for(&file, &[project]).map(|p| path_key(&p)),
            Some(path_key(&root))
        );
    }

    #[test]
    fn a_file_already_covered_needs_no_new_folder() {
        let root = scratch("covered");
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        let file = root.join("src.rs");
        std::fs::write(&file, "").unwrap();
        assert_eq!(workspace_folder_for(&file, &[root.clone()]), None);
        assert!(is_within(&file, &root));
        assert!(!is_within(&root.with_file_name("other"), &root));
    }

    #[test]
    fn toolchain_and_registry_sources_are_never_folders() {
        let root = scratch("sysroot");
        let lib = root.join("lib/rustlib/src/rust/library/std/src");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(root.join("lib/rustlib/src/rust/library/std/Cargo.toml"), "[package]").unwrap();
        let file = lib.join("vec.rs");
        std::fs::write(&file, "").unwrap();
        assert_eq!(workspace_folder_for(&file, &[]), None);
    }

    #[test]
    fn a_loose_file_with_no_manifest_has_no_folder() {
        let root = scratch("loose");
        let file = root.join("script.rs");
        std::fs::write(&file, "").unwrap();
        assert_eq!(workspace_folder_for(&file, &[]), None);
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
