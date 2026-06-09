use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Persisted status of a project on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ProjectStatus {
    /// No sessions active; files are at rest.
    #[default]
    Idle,
    /// Server is loading the project into memory.
    Preparing,
    /// One or more users are actively editing.
    Running,
    /// An error occurred during prepare or operation.
    Error(String),
}

#[allow(dead_code)]
impl ProjectStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProjectStatus::Idle => "idle",
            ProjectStatus::Preparing => "preparing",
            ProjectStatus::Running => "running",
            ProjectStatus::Error(_) => "error",
        }
    }
}

/// A single project record stored in projects.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub owner: String,
    pub created_at: DateTime<Utc>,
    pub last_modified: DateTime<Utc>,
    /// Disk size in bytes, refreshed periodically.
    pub size_bytes: u64,
    /// Runtime status — not persisted; reset to Idle on server start.
    #[serde(skip, default)]
    pub status: ProjectStatus,
    /// Optional error message accompanying `Error` status.
    #[serde(skip, default)]
    pub error_msg: String,
}
