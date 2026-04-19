//! Multiuser Session Context
//!
//! Types for collaborative editing sessions.  The live session state is stored
//! exclusively in `EngineContext::multiuser` — there is no separate global
//! static.  Use `EngineContext::global()` to read or mutate session state.

/// Connection status for multiuser session
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiuserStatus {
    /// Not connected to any session
    Disconnected,
    /// Currently connecting to a session
    Connecting,
    /// Connected and active in a session
    Connected,
    /// Connection error occurred
    Error(String),
}

impl Default for MultiuserStatus {
    fn default() -> Self {
        Self::Disconnected
    }
}

/// Context for an active multiuser session
///
/// Stores all connection details needed by subsystems to participate
/// in collaborative editing.  Stored inside `EngineContext::multiuser`.
#[derive(Clone, Debug)]
pub struct MultiuserContext {
    /// Server WebSocket URL (e.g., "ws://localhost:8080")
    pub server_url: String,
    /// Current session ID
    pub session_id: String,
    /// Our unique peer ID in this session
    pub peer_id: String,
    /// Host's peer ID (the session creator)
    pub host_peer_id: String,
    /// Current connection status
    pub status: MultiuserStatus,
    /// Are we the host of this session?
    pub is_host: bool,
    /// List of other participants (peer IDs)
    pub participants: Vec<String>,
    /// Session join token (for inviting others)
    pub join_token: Option<String>,
    /// Optional Bearer token for the `pulsar-host` file API.
    pub auth_token: Option<String>,
    /// The project UUID on the `pulsar-host` server.
    pub project_id: Option<String>,
}

impl MultiuserContext {
    pub fn new(
        server_url: impl Into<String>,
        session_id: impl Into<String>,
        peer_id: impl Into<String>,
        host_peer_id: impl Into<String>,
    ) -> Self {
        let peer_id_str = peer_id.into();
        let host_peer_id_str = host_peer_id.into();
        let is_host = peer_id_str == host_peer_id_str;
        Self {
            server_url: server_url.into(),
            session_id: session_id.into(),
            peer_id: peer_id_str,
            host_peer_id: host_peer_id_str,
            status: MultiuserStatus::Disconnected,
            is_host,
            participants: Vec::new(),
            join_token: None,
            auth_token: None,
            project_id: None,
        }
    }

    pub fn with_status(mut self, status: MultiuserStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_join_token(mut self, token: impl Into<String>) -> Self {
        self.join_token = Some(token.into());
        self
    }

    pub fn with_participants(mut self, participants: Vec<String>) -> Self {
        self.participants = participants;
        self
    }

    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    pub fn with_project_id(mut self, id: impl Into<String>) -> Self {
        self.project_id = Some(id.into());
        self
    }

    pub fn set_status(&mut self, status: MultiuserStatus) {
        self.status = status;
    }

    pub fn add_participant(&mut self, peer_id: impl Into<String>) {
        let peer_id = peer_id.into();
        if !self.participants.contains(&peer_id) {
            self.participants.push(peer_id);
        }
    }

    pub fn remove_participant(&mut self, peer_id: &str) {
        self.participants.retain(|p| p != peer_id);
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.status, MultiuserStatus::Connected)
    }

    pub fn participant_count(&self) -> usize {
        self.participants.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiuser_context_creation() {
        let ctx = MultiuserContext::new(
            "ws://localhost:8080", "session-123", "peer-abc", "peer-xyz",
        );
        assert_eq!(ctx.server_url, "ws://localhost:8080");
        assert_eq!(ctx.peer_id, "peer-abc");
        assert!(!ctx.is_host);
    }

    #[test]
    fn test_host_detection() {
        let ctx = MultiuserContext::new(
            "ws://localhost:8080", "session-123", "peer-abc", "peer-abc",
        );
        assert!(ctx.is_host);
    }

    #[test]
    fn test_participant_management() {
        let mut ctx = MultiuserContext::new(
            "ws://localhost:8080", "session-123", "peer-abc", "peer-abc",
        );
        ctx.add_participant("peer-def");
        ctx.add_participant("peer-ghi");
        assert_eq!(ctx.participant_count(), 2);
        ctx.remove_participant("peer-def");
        assert_eq!(ctx.participants, vec!["peer-ghi"]);
    }
}
