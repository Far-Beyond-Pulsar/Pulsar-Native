use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub host_id: String,
    pub participants: Vec<ParticipantInfo>,
    pub created_at: u64,
    pub mode: SessionMode,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub peer_id: String,
    pub role: Role,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub joined_at: u64,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    Host,
    Editor,
    Observer,
}

impl Role {
    pub fn can_write(&self) -> bool {
        matches!(self, Role::Host | Role::Editor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionMode {
    Hosted { server_url: String, project_id: String },
    P2P { relay_url: Option<String> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerProfile {
    pub peer_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}
