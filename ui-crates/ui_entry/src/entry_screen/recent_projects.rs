//! Recent Projects Tracking
//!
//! This module provides functionality for tracking and managing recently opened projects.
//!
//! ## Data Structures
//!
//! - `RecentProject` - Individual project information
//! - `RecentProjectsList` - Collection of recent projects (max 20)
//!
//! ## Storage
//!
//! Projects are persisted to disk as JSON in the application data directory.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let mut recent = RecentProjectsList::load(&path);
//! 
//! // Add or update a project
//! recent.add_or_update(RecentProject {
//!     name: "My Game".to_string(),
//!     path: "/path/to/project".to_string(),
//!     last_opened: Some(chrono::Utc::now().to_rfc3339()),
//!     is_git: true,
//! });
//! 
//! // Save to disk
//! recent.save(&path);
//! ```

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub name: String,
    pub path: String,
    pub last_opened: Option<String>,
    pub is_git: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecentProjectsList {
    pub projects: Vec<RecentProject>,
}

impl RecentProjectsList {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn add_or_update(&mut self, project: RecentProject) {
        if let Some(existing) = self.projects.iter_mut().find(|p| p.path == project.path) {
            *existing = project;
        } else {
            self.projects.insert(0, project);
        }
        // Keep only the 20 most recent
        if self.projects.len() > 20 {
            self.projects.truncate(20);
        }
    }

    pub fn remove(&mut self, path: &str) {
        self.projects.retain(|p| p.path != path);
    }
}
