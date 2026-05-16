//! Project Switcher - uses GenericPalette for searchable project selection

use directories::ProjectDirs;
use gpui::{Context, DismissEvent, EventEmitter};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use ui::IconName;
use ui_common::command_palette::{GenericPalette, PaletteDelegate, PaletteItem};

/// Recent project metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub name: String,
    pub path: String,
    pub last_opened: Option<String>,
    pub is_git: bool,
}

/// Recent projects list from disk
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecentProjectsList {
    pub projects: Vec<RecentProject>,
}

impl PaletteItem for RecentProject {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.path
    }

    fn icon(&self) -> IconName {
        if self.is_git {
            IconName::GitBranch
        } else {
            IconName::Folder
        }
    }

    fn keywords(&self) -> Vec<&str> {
        vec!["project", "open"]
    }
}

impl RecentProjectsList {
    pub fn load(path: &std::path::Path) -> Self {
        use ui_common::file_utils;
        file_utils::read_json(path).unwrap_or_default()
    }

    pub fn get_recent_projects_path() -> Option<PathBuf> {
        ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|proj| proj.data_dir().join("recent_projects.json"))
    }

    pub fn load_from_default_location() -> Self {
        Self::get_recent_projects_path()
            .as_ref()
            .map(|p| Self::load(p.as_path()))
            .unwrap_or_default()
    }
}

/// Event emitted when a project is selected
#[derive(Clone)]
pub struct ProjectSelected {
    pub project: RecentProject,
}

/// Delegate for the project switcher palette
pub struct ProjectSwitcherDelegate {
    projects: Vec<RecentProject>,
    pub selected_project: Option<RecentProject>,
}

impl ProjectSwitcherDelegate {
    pub fn new() -> Self {
        let list = RecentProjectsList::load_from_default_location();
        Self {
            projects: list.projects,
            selected_project: None,
        }
    }
}

impl PaletteDelegate for ProjectSwitcherDelegate {
    type Item = RecentProject;

    fn placeholder(&self) -> &str {
        "Search projects..."
    }

    fn categories(&self) -> Vec<(String, Vec<Self::Item>)> {
        // All projects in one "Recent Projects" category
        vec![("Recent Projects".to_string(), self.projects.clone())]
    }

    fn confirm(&mut self, item: &Self::Item) {
        self.selected_project = Some(item.clone());
    }

    fn categories_collapsed_by_default(&self) -> bool {
        false
    }
}

/// Type alias for the project switcher palette view
pub type ProjectSwitcherView = GenericPalette<ProjectSwitcherDelegate>;

impl EventEmitter<ProjectSelected> for ProjectSwitcherView {}
