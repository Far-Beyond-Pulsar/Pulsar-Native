use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// A message that can be sent or received over a project WebSocket session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    /// Client keepalive.
    Ping,
    /// Server keepalive response.
    Pong,
    /// Sent to all existing members when a new user joins.
    UserJoined { user: String },
    /// Sent to all remaining members when a user disconnects.
    UserLeft { user: String },
    /// Full list of currently connected users, sent on join.
    UserList { users: Vec<String> },
    /// An incremental state patch broadcast to all session members.
    StatePatch {
        #[serde(flatten)]
        patch: serde_json::Value,
    },
    /// A chat message (plain text) from a user.
    Chat { user: String, text: String },
    /// Server-level error notification.
    Error { message: String },
    /// A file in the project workspace was created, modified, or deleted.
    ///
    /// `kind` is one of `"created"`, `"modified"`, or `"deleted"`.
    FileChanged { path: String, kind: String },
}

#[allow(dead_code)]
/// A handle to a running collaborative session for one project.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub project_id: String,
    pub tx: broadcast::Sender<WsMessage>,
}

#[allow(dead_code)]
/// Identity of a connected user within a session.
#[derive(Debug, Clone)]
pub struct ConnectedUser {
    pub username: String,
    pub project_id: String,
}
