//! Token persistence for Sketchfab API authentication.
//!
//! Tokens are stored at: <config_dir>/pulsar/sketchfab_token
//! (e.g. ~/.config/pulsar/sketchfab_token on Linux,
//!        %APPDATA%\pulsar\sketchfab_token on Windows)

use std::path::PathBuf;

fn token_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("pulsar")
        .join("sketchfab_token")
}

/// Load the previously saved API token, trimming whitespace.
pub fn load_saved_token() -> Option<String> {
    let path = token_path();
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Persist the API token to disk (creates parent directory as needed).
pub fn save_token(token: &str) {
    let path = token_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, token.trim());
}

/// Delete the persisted token file, if any.
pub fn delete_token() {
    let _ = std::fs::remove_file(token_path());
}
