//! Wire protocol types: ClientMessage, ServerMessage, CandidateDto

use crate::nat::ConnectionCandidate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Join a session
    Join {
        session_id: String,
        peer_id: String,
        join_token: String,
    },
    /// Leave a session
    Leave { session_id: String, peer_id: String },
    /// Kick a user (host only)
    KickUser {
        session_id: String,
        peer_id: String,
        target_peer_id: String,
    },
    /// Chat message
    ChatMessage {
        session_id: String,
        peer_id: String,
        message: String,
    },
    /// Request file manifest from host
    RequestFileManifest { session_id: String, peer_id: String },
    /// Send file manifest (host response)
    FileManifest {
        session_id: String,
        peer_id: String,
        manifest_json: String,
    },
    /// Request specific files
    RequestFiles {
        session_id: String,
        peer_id: String,
        file_paths: Vec<String>,
    },
    /// Send file data chunk
    FilesChunk {
        session_id: String,
        peer_id: String,
        files_json: String,
        chunk_index: usize,
        total_chunks: usize,
    },
    /// P2P connection negotiation
    P2PConnectionRequest {
        session_id: String,
        peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    P2PConnectionResponse {
        session_id: String,
        peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    /// Binary proxy mode (raw bytes, no JSON)
    RequestBinaryProxy { session_id: String, peer_id: String },
    BinaryProxyData {
        session_id: String,
        peer_id: String,
        sequence: u64,
        is_git_protocol: bool,
    },
    /// Git sync messages
    RequestProjectTree { session_id: String, peer_id: String },
    ProjectTreeResponse {
        session_id: String,
        peer_id: String,
        tree_json: String,
    },
    /// Legacy file-based messages
    RequestFile {
        session_id: String,
        peer_id: String,
        file_path: String,
    },
    FileChunk {
        session_id: String,
        peer_id: String,
        file_path: String,
        offset: u64,
        data: Vec<u8>,
        is_last: bool,
    },
    /// Heartbeat / keepalive
    Ping,
}

/// Messages sent FROM server TO client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Confirmation that client joined
    Joined {
        session_id: String,
        peer_id: String,
        participants: Vec<String>,
    },
    /// Another peer joined
    PeerJoined { session_id: String, peer_id: String },
    /// A peer left
    PeerLeft { session_id: String, peer_id: String },
    /// You were kicked from the session
    Kicked { session_id: String, reason: String },
    /// Chat message (relayed)
    ChatMessage {
        session_id: String,
        peer_id: String,
        message: String,
        timestamp: u64,
    },
    /// File manifest request (relayed from guest to host)
    RequestFileManifest {
        session_id: String,
        from_peer_id: String,
    },
    /// File manifest response (relayed from host to guest)
    FileManifest {
        session_id: String,
        from_peer_id: String,
        manifest_json: String,
    },
    /// File request (relayed from guest to host)
    RequestFiles {
        session_id: String,
        from_peer_id: String,
        file_paths: Vec<String>,
    },
    /// Files chunk (relayed from host to guest)
    FilesChunk {
        session_id: String,
        from_peer_id: String,
        files_json: String,
        chunk_index: usize,
        total_chunks: usize,
    },
    /// P2P connection negotiation (relayed)
    P2PConnectionRequest {
        session_id: String,
        from_peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    P2PConnectionResponse {
        session_id: String,
        from_peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    /// Binary proxy (relayed)
    RequestBinaryProxy {
        session_id: String,
        from_peer_id: String,
    },
    BinaryProxyData {
        session_id: String,
        from_peer_id: String,
        sequence: u64,
        is_git_protocol: bool,
    },
    /// Git sync messages (relayed)
    RequestProjectTree {
        session_id: String,
        from_peer_id: String,
    },
    ProjectTreeResponse {
        session_id: String,
        from_peer_id: String,
        tree_json: String,
    },
    /// Legacy file messages
    RequestFile {
        session_id: String,
        from_peer_id: String,
        file_path: String,
    },
    FileChunk {
        session_id: String,
        from_peer_id: String,
        file_path: String,
        offset: u64,
        data: Vec<u8>,
        is_last: bool,
    },
    /// Heartbeat response
    Pong,
    /// Error message
    Error { message: String },
}

/// Candidate DTO for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateDto {
    pub ip: String,
    pub port: u16,
    pub proto: String,
    pub priority: u32,
    pub candidate_type: String,
}

impl From<ConnectionCandidate> for CandidateDto {
    fn from(c: ConnectionCandidate) -> Self {
        Self {
            ip: c.addr.ip().to_string(),
            port: c.addr.port(),
            proto: c.proto,
            priority: c.priority,
            candidate_type: c.candidate_type.to_string(),
        }
    }
}
