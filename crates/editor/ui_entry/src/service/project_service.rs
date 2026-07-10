use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A single recent project entry
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
        ui_common::file_utils::read_json(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let _ = ui_common::file_utils::write_json(path, self);
    }

    pub fn add_or_update(&mut self, project: RecentProject) {
        if let Some(existing) = self.projects.iter_mut().find(|p| p.path == project.path) {
            *existing = project;
        } else {
            self.projects.insert(0, project);
        }
        if self.projects.len() > 20 {
            self.projects.truncate(20);
        }
    }

    pub fn remove(&mut self, path: &str) {
        self.projects.retain(|p| p.path != path);
    }
}

/// Pure functions for project lifecycle
pub struct ProjectService;

impl ProjectService {
    pub fn is_git_repo(path: &Path) -> bool {
        path.join(".git").exists()
    }

    pub fn init_repository(path: &Path) -> Result<git2::Repository, git2::Error> {
        git2::Repository::init(path)
    }

    /// Validate a Pulsar project (contains Pulsar.toml)
    pub fn validate_project(path: &Path) -> bool {
        path.join("Pulsar.toml").exists()
    }

    /// Create a clean Pulsar.toml for a project
    pub fn write_pulsar_toml(path: &Path, name: &str) -> Result<(), std::io::Error> {
        let content = format!(
            r#"[project]
name = "{}"
version = "0.1.0"
engine_version = "0.1.23"

[settings]
default_scene = "scenes/main.scene"
"#,
            name
        );
        std::fs::write(path.join("Pulsar.toml"), content)
    }

    /// Create the standard directory structure for a new project
    pub fn create_project_dirs(path: &Path) -> std::io::Result<()> {
        for dir in &["assets", "scenes", "scripts", "prefabs"] {
            std::fs::create_dir_all(path.join(dir))?;
        }
        Ok(())
    }

    /// Normalize a path that might have a doubled folder name
    pub fn normalize_path(path: &str) -> PathBuf {
        let buf = PathBuf::from(path);
        if let (Some(file_name), Some(parent)) = (buf.file_name(), buf.parent()) {
            if let Some(parent_name) = parent.file_name() {
                if file_name == parent_name {
                    return parent.to_path_buf();
                }
            }
        }
        buf
    }

    /// Read tool preferences from Pulsar.toml
    pub fn load_tool_preferences(project_path: &PathBuf) -> (Option<String>, Option<String>) {
        let config_path = project_path.join("Pulsar.toml");
        if !config_path.exists() {
            return (None, None);
        }
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(parsed) = toml::from_str::<toml::Value>(&content) {
                if let Some(tools) = parsed.get("tools").and_then(|v| v.as_table()) {
                    return (
                        tools
                            .get("editor")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        tools
                            .get("git_tool")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    );
                }
            }
        }
        (None, None)
    }
}
