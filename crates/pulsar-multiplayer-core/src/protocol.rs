use serde::{Deserialize, Serialize};

use crate::session::{FileChangeKind, ManifestEntry};

// ---------------------------------------------------------------------------
// Lifecycle types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    pub session_id: String,
    pub display_name: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinedResponse {
    pub session: crate::session::SessionInfo,
    pub your_peer_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaveRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerJoined {
    pub peer: crate::session::ParticipantInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerLeft {
    pub peer_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kicked {
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Presence types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub text: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorUpdate {
    pub peer_id: String,
    pub path: Option<String>,
    pub line: u32,
    pub column: u32,
}

// ---------------------------------------------------------------------------
// File sync types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChanged {
    pub path: String,
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestFile {
    pub path: String,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    pub path: String,
    pub offset: u64,
    pub data: Vec<u8>,
    pub is_last: bool,
}

// ---------------------------------------------------------------------------
// Replication types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateUpdate {
    pub element_id: String,
    pub state: serde_json::Value,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLock {
    pub element_id: String,
    pub peer_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseLock {
    pub element_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockGranted {
    pub element_id: String,
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockDenied {
    pub element_id: String,
    pub peer_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestPermission {
    pub element_id: String,
    pub permission: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionGranted {
    pub element_id: String,
    pub permission: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDenied {
    pub element_id: String,
    pub permission: String,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// P2P signaling types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConnectionRequest {
    pub session_id: String,
    pub sdp: String,
    pub candidate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConnectionResponse {
    pub session_id: String,
    pub sdp: Option<String>,
    pub candidate: Option<String>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Unified SessionMessage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionMessage {
    // Lifecycle
    Join(JoinRequest),
    Joined(JoinedResponse),
    Leave(LeaveRequest),
    PeerJoined(PeerJoined),
    PeerLeft(PeerLeft),
    Kicked(Kicked),

    // Keepalive
    Ping,
    Pong,

    // Presence
    Chat(ChatMessage),
    CursorUpdate(CursorUpdate),

    // File sync
    FileChanged(FileChanged),
    RequestFileManifest,
    FileManifest(FileManifest),
    RequestFile(RequestFile),
    FileChunk(FileChunk),

    // State replication
    StateUpdate(StateUpdate),
    RequestLock(RequestLock),
    ReleaseLock(ReleaseLock),
    LockGranted(LockGranted),
    LockDenied(LockDenied),
    RequestPermission(RequestPermission),
    PermissionGranted(PermissionGranted),
    PermissionDenied(PermissionDenied),

    // P2P signaling (ignored in hosted mode)
    P2pConnectionRequest(P2pConnectionRequest),
    P2pConnectionResponse(P2pConnectionResponse),

    // Errors
    Error(ProtocolError),
}

impl SessionMessage {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Join(_) => "join",
            Self::Joined(_) => "joined",
            Self::Leave(_) => "leave",
            Self::PeerJoined(_) => "peer_joined",
            Self::PeerLeft(_) => "peer_left",
            Self::Kicked(_) => "kicked",
            Self::Ping => "ping",
            Self::Pong => "pong",
            Self::Chat(_) => "chat",
            Self::CursorUpdate(_) => "cursor_update",
            Self::FileChanged(_) => "file_changed",
            Self::RequestFileManifest => "request_file_manifest",
            Self::FileManifest(_) => "file_manifest",
            Self::RequestFile(_) => "request_file",
            Self::FileChunk(_) => "file_chunk",
            Self::StateUpdate(_) => "state_update",
            Self::RequestLock(_) => "request_lock",
            Self::ReleaseLock(_) => "release_lock",
            Self::LockGranted(_) => "lock_granted",
            Self::LockDenied(_) => "lock_denied",
            Self::RequestPermission(_) => "request_permission",
            Self::PermissionGranted(_) => "permission_granted",
            Self::PermissionDenied(_) => "permission_denied",
            Self::P2pConnectionRequest(_) => "p2p_connection_request",
            Self::P2pConnectionResponse(_) => "p2p_connection_response",
            Self::Error(_) => "error",
        }
    }

    pub fn is_lifecycle(&self) -> bool {
        matches!(
            self,
            Self::Join(_)
                | Self::Joined(_)
                | Self::Leave(_)
                | Self::PeerJoined(_)
                | Self::PeerLeft(_)
                | Self::Kicked(_)
        )
    }

    pub fn is_presence(&self) -> bool {
        matches!(self, Self::Chat(_) | Self::CursorUpdate(_))
    }

    pub fn is_file_sync(&self) -> bool {
        matches!(
            self,
            Self::FileChanged(_)
                | Self::RequestFileManifest
                | Self::FileManifest(_)
                | Self::RequestFile(_)
                | Self::FileChunk(_)
        )
    }

    pub fn is_replication(&self) -> bool {
        matches!(
            self,
            Self::StateUpdate(_)
                | Self::RequestLock(_)
                | Self::ReleaseLock(_)
                | Self::LockGranted(_)
                | Self::LockDenied(_)
                | Self::RequestPermission(_)
                | Self::PermissionGranted(_)
                | Self::PermissionDenied(_)
        )
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}
