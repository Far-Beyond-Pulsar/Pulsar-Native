//! Recent-project bookkeeping — load, save, and update the MRU list.

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecentProject {
    pub name: String,
    pub path: String,
    pub last_opened: Option<String>,
    pub is_git: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RecentProjectsList {
    pub projects: Vec<RecentProject>,
}

impl RecentProjectsList {
    pub(crate) fn load(path: &Path) -> Self {
        use ui_common::file_utils;
        file_utils::read_json(path).unwrap_or_default()
    }

    pub(crate) fn save(&self, path: &Path) {
        use ui_common::file_utils;
        let _ = file_utils::write_json(path, self);
    }

    pub(crate) fn add_or_update(&mut self, project: RecentProject) {
        if let Some(existing) = self.projects.iter_mut().find(|p| p.path == project.path) {
            *existing = project;
        } else {
            self.projects.insert(0, project);
        }
        self.projects.truncate(20);
    }
}

pub(crate) fn update_recent_projects(project_path: &Path) {
    let Some(proj_dirs) = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine") else {
        return;
    };
    let recent_path = proj_dirs.data_dir().join("recent_projects.json");
    let mut list = RecentProjectsList::load(&recent_path);
    list.add_or_update(RecentProject {
        name: project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string(),
        path: project_path.to_string_lossy().to_string(),
        last_opened: Some(chrono::Local::now().to_rfc3339()),
        is_git: project_path.join(".git").exists(),
    });
    list.save(&recent_path);
}
