pub mod sidebar;
pub mod upstream_prompt;
pub mod recent_projects;
pub mod templates;
pub mod new_project;
pub mod clone_git;
pub mod project_settings;
pub mod dependency_setup;

pub use sidebar::render_sidebar;
pub use upstream_prompt::render_upstream_prompt;
pub use recent_projects::render_recent_projects;
pub use templates::render_templates;
pub use new_project::render_new_project;
pub use clone_git::render_clone_git;
pub use project_settings::{
    render_project_settings, 
    ProjectSettings, 
    ProjectSettingsTab,
    types::load_project_tool_preferences,
};
pub use dependency_setup::render_dependency_setup;
