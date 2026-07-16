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

/// Convert an LSP `file://` URI back to a local path string.
///
/// The inverse of [`path_to_uri`]: preserves the root slash of Unix absolute
/// paths (`file:///Users/x` → `/Users/x`), strips the extra slash before
/// Windows drive letters (`file:///C:/x` → `C:/x`), and percent-decodes all
/// escapes, not just `%20`.
pub fn uri_to_path(uri: &str) -> String {
    let raw = uri.strip_prefix("file://").unwrap_or(uri);
    let decoded = percent_decode(raw);

    // Windows drive-letter URIs arrive as /C:/… — drop the authority slash.
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        decoded[1..].to_string()
    } else {
        decoded
    }
}

/// Single-pass percent-decoding; invalid escapes pass through unchanged.
fn percent_decode(input: &str) -> String {
    fn hex_val(b: u8) -> Option<u8> {
        (b as char).to_digit(16).map(|v| v as u8)
    }

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
    fn uri_to_path_preserves_unix_root_slash() {
        // #242: stripping "file:///" ate the leading slash, turning absolute
        // paths relative — the diagnostics panel then couldn't open the file.
        assert_eq!(
            uri_to_path("file:///Users/tristan/project/src/mod.rs"),
            "/Users/tristan/project/src/mod.rs"
        );
    }

    #[test]
    fn uri_to_path_strips_slash_before_windows_drive() {
        assert_eq!(
            uri_to_path("file:///C:/Users/test/file.rs"),
            "C:/Users/test/file.rs"
        );
    }

    #[test]
    fn uri_to_path_decodes_all_percent_escapes() {
        assert_eq!(
            uri_to_path("file:///C%3A/Users/a%20b/file.rs"),
            "C:/Users/a b/file.rs"
        );
        assert_eq!(
            uri_to_path("file:///home/user/caf%C3%A9/%2525.rs"),
            "/home/user/café/%25.rs"
        );
    }

    #[test]
    fn uri_to_path_round_trips_with_path_to_uri() {
        for original in ["/home/user/file.rs", "C:/Users/test/file.rs"] {
            let uri = path_to_uri(&PathBuf::from(original));
            assert_eq!(uri_to_path(&uri), original);
        }
    }

    #[test]
    fn uri_to_path_passes_through_invalid_escapes() {
        assert_eq!(uri_to_path("file:///tmp/50%25done%Zq.rs"), "/tmp/50%done%Zq.rs");
    }
}
