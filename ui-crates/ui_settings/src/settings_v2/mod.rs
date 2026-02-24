mod field_renderers;
mod settings_screen;
mod tabs;

pub use settings_screen::*;
pub use tabs::*;

use engine_state::{GlobalSettings as GlobalSettingsBackend, ProjectSettings as ProjectSettingsBackend};
use gpui::*;
use std::path::PathBuf;

/// Tab type for settings
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Global,
    Project,
}

impl SettingsTab {
    pub fn label(&self) -> &'static str {
        match self {
            SettingsTab::Global => "Global",
            SettingsTab::Project => "Project",
        }
    }

    pub fn icon(&self) -> ui::IconName {
        match self {
            SettingsTab::Global => ui::IconName::Settings,
            SettingsTab::Project => ui::IconName::Folder,
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            SettingsTab::Global => "Engine-wide settings that apply to all projects",
            SettingsTab::Project => "Project-specific settings for the current project",
        }
    }
}

/// Container for both global and project settings
pub struct SettingsContainer {
    pub global: GlobalSettingsBackend,
    pub project: Option<ProjectSettingsBackend>,
}

impl SettingsContainer {
    pub fn new(project_path: Option<PathBuf>) -> Self {
        let global = GlobalSettingsBackend::new();
        let project = project_path.map(|path| ProjectSettingsBackend::new(&path));

        Self { global, project }
    }
}
