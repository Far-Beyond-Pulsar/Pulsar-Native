use parking_lot::Mutex;
use std::sync::Arc;
use ui::IconName;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryScreenView {
    Recent,
    Templates,
    NewProject,
    CloneGit,
    CloudProjects,
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

// ── Cloud Projects ─────────────────────────────────────────────────────────

/// Runtime connection status of a cloud server (not persisted to disk).
#[derive(Clone, Debug, PartialEq)]
pub enum CloudServerStatus {
    /// Initial / never polled
    Unknown,
    /// Poll in progress
    Connecting,
    /// Server replied successfully with these stats
    Online {
        latency_ms: u32,
        version: String,
        active_users: u32,
        active_projects: u32,
    },
    /// Could not reach server
    Offline,
    /// Server returned 401 / 403
    Unauthorized,
}

impl Default for CloudServerStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Status of a single project on a remote server.
#[derive(Clone, Debug, PartialEq)]
pub enum CloudProjectStatus {
    Idle,
    Preparing,
    Running { user_count: u32 },
    Error(String),
}

/// A project hosted on a remote Pulsar Host server.
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

/// A user-configured remote Pulsar Host server entry.
/// Only the four identifying fields are persisted to disk; runtime state is skipped.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CloudServer {
    /// Stable opaque ID generated at creation time
    pub id: String,
    /// Human-friendly label chosen by the user
    pub alias: String,
    /// Base URL, e.g. "https://studio.example.com"
    pub url: String,
    /// Optional bearer token for authenticated servers
    pub auth_token: String,
    /// Connection status — filled in at runtime, not stored
    #[serde(skip)]
    pub status: CloudServerStatus,
    /// Projects currently loaded from this server — not stored
    #[serde(skip)]
    pub projects: Vec<CloudProject>,
}

impl Default for CloudServer {
    fn default() -> Self {
        Self {
            id: String::new(),
            alias: String::new(),
            url: String::new(),
            auth_token: String::new(),
            status: CloudServerStatus::Unknown,
            projects: Vec::new(),
        }
    }
}

/// Get default templates list
pub fn get_default_templates() -> Vec<Template> {
    vec![
        Template::new(
            "Blank Project",
            "Empty project with minimal structure",
            IconName::Folder,
            "https://github.com/Far-Beyond-Pulsar/Template-Blank",
            "Basic",
        ),
        Template::new(
            "Core",
            "Core engine features and systems",
            IconName::Settings,
            "https://github.com/pulsar-templates/core.git",
            "Basic",
        ),
        Template::new(
            "2D Platformer",
            "Classic side-scrolling platformer",
            IconName::Gamepad,
            "https://github.com/pulsar-templates/2d-platformer.git",
            "2D",
        ),
        Template::new(
            "2D Top-Down",
            "Top-down 2D game with camera",
            IconName::Map,
            "https://github.com/pulsar-templates/2d-topdown.git",
            "2D",
        ),
        Template::new(
            "3D First Person",
            "FPS with movement and camera",
            IconName::Eye,
            "https://github.com/pulsar-templates/3d-fps.git",
            "3D",
        ),
        Template::new(
            "3D Platformer",
            "3D platformer with physics",
            IconName::Cube,
            "https://github.com/pulsar-templates/3d-platformer.git",
            "3D",
        ),
        Template::new(
            "Tower Defense",
            "Wave-based tower defense",
            IconName::Shield,
            "https://github.com/pulsar-templates/tower-defense.git",
            "Strategy",
        ),
        Template::new(
            "Action RPG",
            "Action-oriented RPG systems",
            IconName::Star,
            "https://github.com/pulsar-templates/action-rpg.git",
            "RPG",
        ),
        Template::new(
            "Visual Novel",
            "Narrative-driven visual novel",
            IconName::BookOpen,
            "https://github.com/pulsar-templates/visual-novel.git",
            "Narrative",
        ),
        Template::new(
            "Puzzle",
            "Puzzle game mechanics",
            IconName::Box,
            "https://github.com/pulsar-templates/puzzle.git",
            "Puzzle",
        ),
        Template::new(
            "Card Game",
            "Card-based game system",
            IconName::CreditCard,
            "https://github.com/pulsar-templates/card-game.git",
            "Card",
        ),
        Template::new(
            "Racing",
            "Racing game with physics",
            IconName::Rocket,
            "https://github.com/pulsar-templates/racing.git",
            "Racing",
        ),
    ]
}
