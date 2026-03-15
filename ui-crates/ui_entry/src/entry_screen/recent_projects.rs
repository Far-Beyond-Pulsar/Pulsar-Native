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
        use ui_common::file_utils;
        file_utils::read_json(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        use ui_common::file_utils;
        let _ = file_utils::write_json(path, self);
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
