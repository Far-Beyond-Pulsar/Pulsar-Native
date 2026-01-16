//! App data and resource directory management for Pulsar Engine
//
// This module handles creation and management of app data directories, themes, and config files.

use directories::ProjectDirs;
use std::fs;
use std::path::{Path, PathBuf};
use crate::settings::EngineSettings;

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

    // Extract bundled themes if not present
    if !themes_dir.exists() {
        if let Err(e) = fs::create_dir_all(&themes_dir) {
            tracing::error!("Failed to create themes directory: {e}");
        } else {
            let project_themes_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent().unwrap().join("themes");
            if let Ok(entries) = fs::read_dir(&project_themes_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(name) = path.file_name() {
                            let dest = themes_dir.join(name);
                            let _ = fs::copy(&path, &dest);
                        }
                    }
                }
            }
        }
    }

    // Create default config if not present
    if !config_file.exists() {
        if let Err(e) = fs::create_dir_all(&config_dir) {
            tracing::error!("Failed to create config directory: {e}");
        }
        let default_settings = EngineSettings::default();
        default_settings.save(&config_file);
    }

    AppDataPaths { appdata_dir, themes_dir, config_dir, config_file }
}
