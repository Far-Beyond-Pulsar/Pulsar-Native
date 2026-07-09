//! Multiuser Session Context
//!
//! Types for collaborative editing sessions.  The live session state is stored
//! exclusively in `EngineContext::multiuser` — there is no separate global
//! static.  Use `EngineContext::global()` to read or mutate session state.

/// Relay/data connection mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayConnectionMode {
    /// Attempting direct P2P connection with hole punching
    DirectP2P,
    /// Using server binary proxy for data relay
    BinaryProxy,
    /// Fallback to JSON messages (degraded mode)
    JsonFallback,
}

/// Connection status for multiuser session
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiuserStatus {
    /// Not connected to any session
    Disconnected,
    /// Currently connecting to a session
    Connecting,
    /// Connected to signaling server and relay/data connection is active
    Connected {
        /// Current relay connection mode (P2P, proxy, or fallback)
        relay_mode: Option<RelayConnectionMode>,
    },
    /// Signaling connected but data connection failed, using fallback
    DegradedMode {
        /// Which fallback mode we're using
        relay_mode: RelayConnectionMode,
    },
    /// Connection error occurred
    Error(String),
}

impl Default for MultiuserStatus {
    fn default() -> Self {
        MultiuserStatus::Disconnected
    }
}

/// Top-level collaboration mode.
///
/// The engine supports two runtime collaboration paths:
/// - `CloudProject`: dedicated host service/project-backed collaboration
/// - `PeerToPeer`: direct peer session via the multiplayer server path
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MultiuserMode {
    /// Dedicated host / cloud project mode.
    CloudProject,
    /// Traditional P2P session mode.
    #[default]
    PeerToPeer,
}

/// Context for an active multiuser session
///
/// Stores all connection details needed by subsystems to participate
/// in collaborative editing.  Stored inside `EngineContext::multiuser`.
#[derive(Clone, Debug)]
pub struct MultiuserContext {
    /// Active collaboration mode.
    pub mode: MultiuserMode,
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
    /// Rich participant metadata when available.
    pub participant_profiles: Vec<MultiuserParticipant>,
    /// Last measured latency to signaling server in milliseconds.
    pub latency_ms: Option<u32>,
    /// Session join token (for inviting others)
    pub join_token: Option<String>,
    /// Optional Bearer token for the `pulsar-studio` file API.
    pub auth_token: Option<String>,
    /// The workspace UUID on the `pulsar-studio` server.
    pub workspace_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MultiuserParticipant {
    pub peer_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub github_login: Option<String>,
    pub ping_ms: Option<u32>,
}

impl MultiuserContext {
    /// Construct a P2P session context.
    pub fn new_peer_to_peer(
        server_url: impl Into<String>,
        session_id: impl Into<String>,
        peer_id: impl Into<String>,
        host_peer_id: impl Into<String>,
    ) -> Self {
        Self::new(server_url, session_id, peer_id, host_peer_id)
            .with_mode(MultiuserMode::PeerToPeer)
    }

    /// Construct a cloud project session context.
    pub fn new_cloud_project(
        server_url: impl Into<String>,
        workspace_id: impl Into<String>,
        peer_id: impl Into<String>,
        host_peer_id: impl Into<String>,
    ) -> Self {
        let workspace_id = workspace_id.into();
        Self::new(server_url, workspace_id.clone(), peer_id, host_peer_id)
            .with_mode(MultiuserMode::CloudProject)
            .with_workspace_id(workspace_id)
    }

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
            mode: MultiuserMode::PeerToPeer,
            server_url: server_url.into(),
            session_id: session_id.into(),
            peer_id: peer_id_str,
            host_peer_id: host_peer_id_str,
            status: MultiuserStatus::Disconnected,
            is_host,
            participants: Vec::new(),
            participant_profiles: Vec::new(),
            latency_ms: None,
            join_token: None,
            auth_token: None,
            workspace_id: None,
        }
    }

    pub fn with_mode(mut self, mode: MultiuserMode) -> Self {
        self.mode = mode;
        self
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

    pub fn with_participant_profiles(mut self, participants: Vec<MultiuserParticipant>) -> Self {
        self.participant_profiles = participants;
        self
    }

    pub fn with_latency_ms(mut self, latency_ms: u32) -> Self {
        self.latency_ms = Some(latency_ms);
        self
    }

    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    pub fn with_workspace_id(mut self, id: impl Into<String>) -> Self {
        self.workspace_id = Some(id.into());
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
        matches!(
            self.status,
            MultiuserStatus::Connected { .. } | MultiuserStatus::DegradedMode { .. }
        )
    }

    pub fn participant_count(&self) -> usize {
        if self.participant_profiles.is_empty() {
            self.participants.len()
        } else {
            self.participant_profiles.len()
        }
    }

    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            MultiuserMode::CloudProject => "Cloud",
            MultiuserMode::PeerToPeer => "P2P",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiuser_context_creation() {
        let ctx =
            MultiuserContext::new("ws://localhost:8080", "session-123", "peer-abc", "peer-xyz");
        assert_eq!(ctx.server_url, "ws://localhost:8080");
        assert_eq!(ctx.peer_id, "peer-abc");
        assert!(!ctx.is_host);
    }

    #[test]
    fn test_host_detection() {
        let ctx =
            MultiuserContext::new("ws://localhost:8080", "session-123", "peer-abc", "peer-abc");
        assert!(ctx.is_host);
    }

    #[test]
    fn test_participant_management() {
        let mut ctx =
            MultiuserContext::new("ws://localhost:8080", "session-123", "peer-abc", "peer-abc");
        ctx.add_participant("peer-def");
        ctx.add_participant("peer-ghi");
        assert_eq!(ctx.participant_count(), 2);
        ctx.remove_participant("peer-def");
        assert_eq!(ctx.participants, vec!["peer-ghi"]);
    }
}
