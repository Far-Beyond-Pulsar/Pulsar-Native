//! App data and resource directory management for Pulsar Engine
//
// This module handles creation and management of app data directories, themes, and config files.

use crate::settings::EngineSettings;
use directories::ProjectDirs;
use rust_embed::RustEmbed;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Themes shipped with the engine, embedded into the binary at compile time.
///
/// Embedding (rather than reading a source-tree path) is what lets the engine seed
/// and update its own theme cache on *any* machine, not just a dev checkout.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../../themes"]
#[include = "*.json"]
struct BundledThemes;

pub struct AppDataPaths {
    pub appdata_dir: PathBuf,
    pub themes_dir: PathBuf,
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
}

/// Set up app data directories and copy bundled resources if needed.
pub fn setup_appdata() -> AppDataPaths {
    let proj_dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .expect("Could not determine app data directory");
    let appdata_dir = proj_dirs.data_dir().to_path_buf();
    let themes_dir = appdata_dir.join("themes");
    let config_dir = appdata_dir.join("configs");
    let config_file = config_dir.join("engine.toml");

    // Sync the engine's bundled themes into the user's theme cache, updating our own
    // themes while leaving anything the user added or edited untouched.
    if let Err(e) = fs::create_dir_all(&themes_dir) {
        tracing::error!("Failed to create themes directory: {e}");
    } else {
        let manifest_path = appdata_dir.join(".themes_cache.json");
        sync_bundled_themes(&themes_dir, &manifest_path);
    }

    // Create default config if not present
    if !config_file.exists() {
        if let Err(e) = fs::create_dir_all(&config_dir) {
            tracing::error!("Failed to create config directory: {e}");
        }
        let default_settings = EngineSettings::default();
        default_settings.save(&config_file);
    }

    AppDataPaths {
        appdata_dir,
        themes_dir,
        config_dir,
        config_file,
    }
}

/// Stable, dependency-free content hash (FNV-1a, 64-bit) used to detect whether a
/// cached theme file still matches the version the engine last wrote.
fn content_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Synchronise the engine's bundled themes into the user's theme cache.
///
/// A small manifest (`{filename -> hash the engine last wrote}`) is kept next to the
/// cache so we can tell engine-owned files apart from the user's. The rules:
///
/// - **New** bundled themes are written into the cache.
/// - Engine-owned themes the user has **not** edited are refreshed to the current
///   shipped version (this is the "update its own cache" behaviour).
/// - A cache file that no longer matches what we last shipped is treated as
///   **user-edited** and left completely untouched.
/// - Files the user **added themselves** (not part of the bundle) are never touched.
/// - Bundled themes **removed** in an engine update are deleted only if the user
///   never edited them; edited copies are preserved.
///
/// The manifest is stored in `appdata_dir` (not `themes_dir`) so the theme watcher,
/// which loads every `*.json` in `themes_dir`, never tries to parse it as a theme.
fn sync_bundled_themes(themes_dir: &Path, manifest_path: &Path) {
    // Manifest of what the engine last wrote: filename -> content hash.
    let manifest: HashMap<String, String> = fs::read_to_string(manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let mut next_manifest: HashMap<String, String> = HashMap::new();
    let mut bundled_names: HashSet<String> = HashSet::new();

    for file in BundledThemes::iter() {
        let name = file.as_ref().to_string();
        // Only handle flat, top-level `*.json` theme files.
        if !name.ends_with(".json") || name.contains('/') || name.contains('\\') {
            continue;
        }
        let Some(embedded) = BundledThemes::get(&name) else {
            continue;
        };
        let new_bytes = embedded.data.as_ref();
        let new_hash = content_hash(new_bytes);
        bundled_names.insert(name.clone());

        let dest = themes_dir.join(&name);
        let prev_hash = manifest.get(&name).cloned();

        // Brand-new theme (or one a previous engine version had removed): seed it.
        if !dest.exists() {
            match fs::write(&dest, new_bytes) {
                Ok(_) => {
                    next_manifest.insert(name, new_hash);
                }
                Err(e) => tracing::error!("Failed to write bundled theme {name}: {e}"),
            }
            continue;
        }

        // Compare the cached file against what we last shipped.
        let disk_hash = match fs::read(&dest) {
            Ok(bytes) => content_hash(&bytes),
            Err(e) => {
                tracing::warn!("Failed to read cached theme {name}, refreshing it: {e}");
                let _ = fs::write(&dest, new_bytes);
                next_manifest.insert(name, new_hash);
                continue;
            }
        };

        // Engine-owned & untouched if it still matches our last write, or (for caches
        // that predate the manifest) if it's byte-identical to the current bundle.
        let engine_owned_unmodified = match &prev_hash {
            Some(prev) => *prev == disk_hash,
            None => disk_hash == new_hash,
        };

        if engine_owned_unmodified {
            if disk_hash != new_hash {
                if let Err(e) = fs::write(&dest, new_bytes) {
                    tracing::error!("Failed to update bundled theme {name}: {e}");
                }
            }
            next_manifest.insert(name, new_hash);
        } else {
            // User created or edited this file — never overwrite it. Keep remembering
            // our last shipped hash (if any) so future updates still detect the edit.
            if let Some(prev) = prev_hash {
                next_manifest.insert(name, prev);
            }
        }
    }

    // Clean up engine themes that are no longer bundled, but only when the user never
    // edited them (i.e. the cache still matches the hash we last wrote).
    for (name, prev_hash) in manifest.iter() {
        if bundled_names.contains(name) {
            continue;
        }
        let dest = themes_dir.join(name);
        match fs::read(&dest) {
            Ok(bytes) if content_hash(&bytes) == *prev_hash => {
                if let Err(e) = fs::remove_file(&dest) {
                    tracing::warn!("Failed to remove retired theme {name}: {e}");
                }
            }
            // Edited by the user, or already gone — leave it and stop tracking it.
            _ => {}
        }
    }

    // Persist the refreshed manifest.
    match serde_json::to_string_pretty(&next_manifest) {
        Ok(json) => {
            if let Err(e) = fs::write(manifest_path, json) {
                tracing::error!("Failed to write theme cache manifest: {e}");
            }
        }
        Err(e) => tracing::error!("Failed to serialize theme cache manifest: {e}"),
    }
}
