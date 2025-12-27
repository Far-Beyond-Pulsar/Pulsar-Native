use ui::IconName;
use std::sync::Arc;
use parking_lot::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryScreenView {
    Recent,
    Templates,
    NewProject,
    CloneGit,
}

/// Template definition with Git repository info
#[derive(Clone)]
pub struct Template {
    pub name: String,
    pub description: String,
    pub icon: IconName,
    pub repo_url: String,
    pub category: String,
}

impl Template {
    pub fn new(name: &str, desc: &str, icon: IconName, repo_url: &str, category: &str) -> Self {
        Self {
            name: name.to_string(),
            description: desc.to_string(),
            icon,
            repo_url: repo_url.to_string(),
            category: category.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct CloneProgress {
    pub current: usize,
    pub total: usize,
    pub message: String,
    pub completed: bool,
    pub error: Option<String>,
}

pub type SharedCloneProgress = Arc<Mutex<CloneProgress>>;

/// Git fetch status for a project
#[derive(Clone)]
pub enum GitFetchStatus {
    NotStarted,
    Fetching,
    UpToDate,
    UpdatesAvailable(usize), // number of commits behind
    Error(String),
}

/// Project with git fetch status
#[derive(Clone)]
pub struct ProjectWithGitStatus {
    pub name: String,
    pub path: String,
    pub last_opened: Option<String>,
    pub is_git: bool,
    pub fetch_status: GitFetchStatus,
}

/// Get default templates list
pub fn get_default_templates() -> Vec<Template> {
    vec![
        Template::new("Blank Project", "Empty project with minimal structure", IconName::Folder, "https://github.com/Far-Beyond-Pulsar/Template-Blank", "Basic"),
        Template::new("Core", "Core engine features and systems", IconName::Settings, "https://github.com/pulsar-templates/core.git", "Basic"),
        Template::new("2D Platformer", "Classic side-scrolling platformer", IconName::Gamepad, "https://github.com/pulsar-templates/2d-platformer.git", "2D"),
        Template::new("2D Top-Down", "Top-down 2D game with camera", IconName::Map, "https://github.com/pulsar-templates/2d-topdown.git", "2D"),
        Template::new("3D First Person", "FPS with movement and camera", IconName::Eye, "https://github.com/pulsar-templates/3d-fps.git", "3D"),
        Template::new("3D Platformer", "3D platformer with physics", IconName::Cube, "https://github.com/pulsar-templates/3d-platformer.git", "3D"),
        Template::new("Tower Defense", "Wave-based tower defense", IconName::Shield, "https://github.com/pulsar-templates/tower-defense.git", "Strategy"),
        Template::new("Action RPG", "Action-oriented RPG systems", IconName::Star, "https://github.com/pulsar-templates/action-rpg.git", "RPG"),
        Template::new("Visual Novel", "Narrative-driven visual novel", IconName::BookOpen, "https://github.com/pulsar-templates/visual-novel.git", "Narrative"),
        Template::new("Puzzle", "Puzzle game mechanics", IconName::Box, "https://github.com/pulsar-templates/puzzle.git", "Puzzle"),
        Template::new("Card Game", "Card-based game system", IconName::CreditCard, "https://github.com/pulsar-templates/card-game.git", "Card"),
        Template::new("Racing", "Racing game with physics", IconName::Rocket, "https://github.com/pulsar-templates/racing.git", "Racing"),
    ]
}
