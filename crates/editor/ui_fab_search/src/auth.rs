//! Token persistence for Sketchfab API authentication.
//!
//! Tokens are stored at: <config_dir>/pulsar/sketchfab_token
//! (e.g. ~/.config/pulsar/sketchfab_token on Linux,
//!        %APPDATA%\pulsar\sketchfab_token on Windows)

use engine_fs::{FsProvider as _, LocalFsProvider};
use std::path::PathBuf;

// The token lives in the user's config dir, outside any project: always the local disk,
// never the active project provider.
fn local() -> LocalFsProvider {
    LocalFsProvider::new()
}

fn token_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("pulsar")
        .join("sketchfab_token")
}

/// Load the previously saved API token, trimming whitespace.
pub fn load_saved_token() -> Option<String> {
    let path = token_path();
    local()
        .read_file(&path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Persist the API token to disk (creates parent directory as needed).
pub fn save_token(token: &str) {
    let path = token_path();
    let _ = local().write_file(&path, token.trim().as_bytes());
}

/// Delete the persisted token file, if any.
pub fn delete_token() {
    let _ = local().delete_path(&token_path());
}
