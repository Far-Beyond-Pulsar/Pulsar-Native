//! Type definitions for the multiplayer window

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
    Presence, // Who's editing what - VSCode LiveShare style
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
    pub hash: String, // SHA-256 hash for verification
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
    pub peers_with_file: Vec<String>, // Which peers have this file
}

#[derive(Clone, Debug)]
pub struct UserPresence {
    pub peer_id: String,
    pub current_tab: Option<String>, // What tab they're viewing
    pub editing_file: Option<String>, // What file they're editing
    pub selected_object: Option<String>, // What object they have selected in scene
    pub cursor_position: Option<(f32, f32, f32)>, // 3D cursor position in scene
    pub last_activity: u64, // Timestamp of last activity
    pub is_idle: bool, // Whether user is idle (no activity for X seconds)
    pub color: [f32; 3], // RGB color to identify this user
}

impl UserPresence {
    pub fn new(peer_id: String) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Generate a unique color based on peer_id
        let hash = peer_id.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
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
