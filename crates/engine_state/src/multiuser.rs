//! Multiuser Session Context
//!
//! Provides centralized, thread-safe access to multiuser session state.
//! Similar to ProjectContext, but for collaborative editing sessions.

use std::sync::{Arc, OnceLock};
use parking_lot::RwLock;

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
/// in collaborative editing.
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
}

impl MultiuserContext {
    /// Create a new multiuser context for a session
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
        }
    }

    /// Builder: Set connection status
    pub fn with_status(mut self, status: MultiuserStatus) -> Self {
        self.status = status;
        self
    }

    /// Builder: Set join token
    pub fn with_join_token(mut self, token: impl Into<String>) -> Self {
        self.join_token = Some(token.into());
        self
    }

    /// Builder: Set participants list
    pub fn with_participants(mut self, participants: Vec<String>) -> Self {
        self.participants = participants;
        self
    }

    /// Update connection status
    pub fn set_status(&mut self, status: MultiuserStatus) {
        self.status = status;
    }

    /// Add a participant to the session
    pub fn add_participant(&mut self, peer_id: impl Into<String>) {
        let peer_id = peer_id.into();
        if !self.participants.contains(&peer_id) {
            self.participants.push(peer_id);
        }
    }

    /// Remove a participant from the session
    pub fn remove_participant(&mut self, peer_id: &str) {
        self.participants.retain(|p| p != peer_id);
    }

    /// Check if we're currently connected
    pub fn is_connected(&self) -> bool {
        matches!(self.status, MultiuserStatus::Connected)
    }

    /// Get participant count (excluding ourselves)
    pub fn participant_count(&self) -> usize {
        self.participants.len()
    }
}

// ============================================================================
// Global Static Access (for backward compatibility and convenience)
// ============================================================================

static MULTIUSER_CONTEXT: OnceLock<Arc<RwLock<Option<MultiuserContext>>>> = OnceLock::new();

/// Get the global multiuser context storage
fn multiuser_storage() -> &'static Arc<RwLock<Option<MultiuserContext>>> {
    MULTIUSER_CONTEXT.get_or_init(|| Arc::new(RwLock::new(None)))
}

/// Set the active multiuser session context
///
/// # Example
/// ```ignore
/// use engine_state::set_multiuser_context;
///
/// let ctx = MultiuserContext::new(
///     "ws://localhost:8080",
///     "session-123",
///     "peer-abc",
///     "peer-abc" // We're the host
/// );
/// set_multiuser_context(ctx);
/// ```
pub fn set_multiuser_context(context: MultiuserContext) {
    tracing::info!("Multiuser context set: session_id={}, peer_id={}, is_host={}",
        context.session_id, context.peer_id, context.is_host);
    *multiuser_storage().write() = Some(context);
}

/// Clear the active multiuser session context
///
/// Call this when disconnecting from a session.
pub fn clear_multiuser_context() {
    *multiuser_storage().write() = None;
    tracing::info!("Multiuser context cleared");
}

/// Get a clone of the current multiuser context
///
/// Returns `None` if not in a session.
///
/// # Example
/// ```ignore
/// if let Some(ctx) = get_multiuser_context() {
///     println!("In session: {}", ctx.session_id);
///     println!("We are {}", if ctx.is_host { "host" } else { "client" });
/// }
/// ```
pub fn get_multiuser_context() -> Option<MultiuserContext> {
    multiuser_storage().read().clone()
}

/// Check if currently in a multiuser session
pub fn is_multiuser_active() -> bool {
    multiuser_storage().read().is_some()
}

/// Check if we're the host of the current session
pub fn are_we_host() -> bool {
    multiuser_storage()
        .read()
        .as_ref()
        .map(|ctx| ctx.is_host)
        .unwrap_or(false)
}

/// Get our peer ID in the current session
pub fn our_peer_id() -> Option<String> {
    multiuser_storage()
        .read()
        .as_ref()
        .map(|ctx| ctx.peer_id.clone())
}

/// Get the host's peer ID
pub fn host_peer_id() -> Option<String> {
    multiuser_storage()
        .read()
        .as_ref()
        .map(|ctx| ctx.host_peer_id.clone())
}

/// Get the current session ID
pub fn session_id() -> Option<String> {
    multiuser_storage()
        .read()
        .as_ref()
        .map(|ctx| ctx.session_id.clone())
}

/// Get the server URL
pub fn server_url() -> Option<String> {
    multiuser_storage()
        .read()
        .as_ref()
        .map(|ctx| ctx.server_url.clone())
}

/// Get the current connection status
pub fn multiuser_status() -> MultiuserStatus {
    multiuser_storage()
        .read()
        .as_ref()
        .map(|ctx| ctx.status.clone())
        .unwrap_or(MultiuserStatus::Disconnected)
}

/// Update the connection status
pub fn set_multiuser_status(status: MultiuserStatus) {
    if let Some(ctx) = multiuser_storage().write().as_mut() {
        ctx.set_status(status);
    }
}

/// Add a participant to the current session
pub fn add_participant(peer_id: impl Into<String>) {
    if let Some(ctx) = multiuser_storage().write().as_mut() {
        ctx.add_participant(peer_id);
    }
}

/// Remove a participant from the current session
pub fn remove_participant(peer_id: &str) {
    if let Some(ctx) = multiuser_storage().write().as_mut() {
        ctx.remove_participant(peer_id);
    }
}

/// Get list of all participants (excluding ourselves)
pub fn get_participants() -> Vec<String> {
    multiuser_storage()
        .read()
        .as_ref()
        .map(|ctx| ctx.participants.clone())
        .unwrap_or_default()
}

/// Get participant count (excluding ourselves)
pub fn participant_count() -> usize {
    multiuser_storage()
        .read()
        .as_ref()
        .map(|ctx| ctx.participant_count())
        .unwrap_or(0)
}

/// Update context with information from EngineContext
///
/// This keeps multiuser context synchronized with the main engine state.
/// Call this when multiuser state changes in EngineContext.
pub fn sync_from_engine_context() {
    if let Some(engine_ctx) = crate::EngineContext::global() {
        if let Some(mu_ctx) = engine_ctx.multiuser.read().clone() {
            set_multiuser_context(mu_ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiuser_context_creation() {
        let ctx = MultiuserContext::new(
            "ws://localhost:8080",
            "session-123",
            "peer-abc",
            "peer-xyz"
        );

        assert_eq!(ctx.server_url, "ws://localhost:8080");
        assert_eq!(ctx.session_id, "session-123");
        assert_eq!(ctx.peer_id, "peer-abc");
        assert_eq!(ctx.host_peer_id, "peer-xyz");
        assert!(!ctx.is_host);
        assert_eq!(ctx.status, MultiuserStatus::Disconnected);
    }

    #[test]
    fn test_host_detection() {
        let ctx = MultiuserContext::new(
            "ws://localhost:8080",
            "session-123",
            "peer-abc",
            "peer-abc" // Same as peer_id = we're the host
        );

        assert!(ctx.is_host);
    }

    #[test]
    fn test_participant_management() {
        let mut ctx = MultiuserContext::new(
            "ws://localhost:8080",
            "session-123",
            "peer-abc",
            "peer-abc"
        );

        ctx.add_participant("peer-def");
        ctx.add_participant("peer-ghi");
        assert_eq!(ctx.participant_count(), 2);

        ctx.remove_participant("peer-def");
        assert_eq!(ctx.participant_count(), 1);
        assert_eq!(ctx.participants, vec!["peer-ghi"]);
    }

    #[test]
    fn test_global_access() {
        clear_multiuser_context();
        assert!(!is_multiuser_active());

        let ctx = MultiuserContext::new(
            "ws://localhost:8080",
            "session-123",
            "peer-abc",
            "peer-abc"
        );

        set_multiuser_context(ctx);
        assert!(is_multiuser_active());
        assert!(are_we_host());
        assert_eq!(our_peer_id(), Some("peer-abc".to_string()));
        assert_eq!(session_id(), Some("session-123".to_string()));

        clear_multiuser_context();
        assert!(!is_multiuser_active());
    }
}
