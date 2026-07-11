use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionTab {
    Info,
    Chat,
    FileSync,
    Presence,
}

#[derive(Clone, Debug)]
pub struct ActiveSession {
    pub session_id: String,
    pub join_token: String,
    pub server_address: String,
    pub connected_users: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub peer_id: String,
    pub message: String,
    pub timestamp: u64,
    pub is_self: bool,
}

#[derive(Clone, Debug)]
pub struct FileAsset {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub last_modified: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FileSyncStatus {
    Synced,
    OutOfSync,
    Missing,
    Checking,
}

#[derive(Clone, Debug)]
pub struct FileAssetStatus {
    pub asset: FileAsset,
    pub status: FileSyncStatus,
    pub peers_with_file: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct UserPresence {
    pub peer_id: String,
    pub current_tab: Option<String>,
    pub editing_file: Option<String>,
    pub selected_object: Option<String>,
    pub cursor_position: Option<(f32, f32, f32)>,
    pub last_activity: u64,
    pub is_idle: bool,
    pub color: [f32; 3],
}

impl UserPresence {
    pub fn new(peer_id: String) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let hash = peer_id
            .bytes()
            .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
        let r = ((hash & 0xFF) as f32) / 255.0;
        let g = (((hash >> 8) & 0xFF) as f32) / 255.0;
        let b = (((hash >> 16) & 0xFF) as f32) / 255.0;

        Self {
            peer_id,
            current_tab: None,
            editing_file: None,
            selected_object: None,
            cursor_position: None,
            last_activity: now,
            is_idle: false,
            color: [r, g, b],
        }
    }

    pub fn activity_status(&self) -> &str {
        if self.is_idle {
            "Idle"
        } else if self.editing_file.is_some() {
            "Editing"
        } else if self.current_tab.is_some() {
            "Active"
        } else {
            "Online"
        }
    }
}
