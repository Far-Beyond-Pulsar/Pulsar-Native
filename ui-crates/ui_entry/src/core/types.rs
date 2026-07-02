use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use ui::IconName;

// ── Navigation ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryScreenView {
    Recent,
    Templates,
    NewProject,
    CloneGit,
    CloudProjects,
    Friends,
}

// ── Templates ─────────────────────────────────────────────────────────────

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

// TODO: Consider loading templates from a remote repo in the github org
//       instead of hardcoding them here. This would allow for easier updates
//       and additions to the template list without requiring a new release of
//       the application.
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

// ── Clone Progress ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CloneProgress {
    pub current: usize,
    pub total: usize,
    pub message: String,
    pub completed: bool,
    pub error: Option<String>,
}

pub type SharedCloneProgress = Arc<Mutex<CloneProgress>>;

// ── Git Fetch Status ──────────────────────────────────────────────────────

#[derive(Clone)]
pub enum GitFetchStatus {
    NotStarted,
    Fetching,
    UpToDate,
    UpdatesAvailable(usize),
    Error(String),
}

// ── Cloud Projects ────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Default)]
pub enum CloudServerStatus {
    #[default]
    Unknown,
    Connecting,
    Online { latency_ms: u32, version: String, active_users: u32, active_projects: u32 },
    Offline,
    Unauthorized,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CloudProjectStatus {
    Idle,
    Preparing,
    Running { user_count: u32 },
    Error(String),
}

#[derive(Clone, Debug)]
pub struct CloudProject {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: CloudProjectStatus,
    pub last_modified: String,
    pub size_bytes: u64,
    pub owner: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CloudServer {
    pub id: String,
    pub alias: String,
    pub url: String,
    pub auth_token: String,
    #[serde(skip)]
    pub status: CloudServerStatus,
    #[serde(skip)]
    pub projects: Vec<CloudProject>,
}

impl Default for CloudServer {
    fn default() -> Self {
        Self { id: String::new(), alias: String::new(), url: String::new(), auth_token: String::new(), status: CloudServerStatus::Unknown, projects: Vec::new() }
    }
}

// ── Dependency Status ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct DependencyStatus {
    pub rust_installed: bool,
    pub build_tools_installed: bool,
    pub compiler_info: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InstallProgress {
    pub logs: Vec<String>,
    pub progress: f32,
    pub status: InstallStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InstallStatus {
    Idle, Downloading, Installing, Complete, Error(String),
}

// ── Plugin System ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PluginRegistry {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RegistryPlugin {
    pub name: String,
    pub description: String,
    pub repo_url: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip)]
    pub registry_url: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum PluginInstallMethod { BinaryDownload, BuiltFromSource }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub repo_url: String,
    pub version: String,
    pub installed_at: String,
    pub install_method: PluginInstallMethod,
    pub library_path: String,
}

#[derive(Clone, Debug)]
pub enum PluginInstallPhase {
    FetchingMetadata,
    Downloading { progress: f32 },
    Building { logs: Vec<String> },
    Complete(InstalledPlugin),
    Error(String),
}

// ── Onboarding ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum OnboardingTab { #[default] Theme, Plugins }

// ── Pending Invite ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PendingInvite {
    pub from_username: String,
    pub from_home_server: Option<String>,
    pub message: String,
    pub notification_id: String,
}
