//! Integration layer for connecting replication to the multiuser client

use crate::{
    ReplicationMessage, ReplicationMessageHandler, ReplicationRegistry, SessionContext,
    UserPresence,
};
use gpui::App;

/// Helper to integrate replication with your multiuser client
///
/// Provides the connection between UI state replication and network transport.
pub struct MultiuserIntegration;

impl MultiuserIntegration {
    /// Create a new integration instance.
    /// Panics if [`crate::init`] has not been called first.
    pub fn new(cx: &App) -> Self {
        if cx.try_global::<SessionContext>().is_none() {
            panic!("SessionContext not initialized. Call pulsar_replication::init(cx) first.");
        }

        Self
    }

    /// Start a multiuser session with replication support
    pub fn start_session<F>(
        &self,
        our_peer_id: String,
        host_peer_id: String,
        send_callback: F,
        cx: &App,
    ) where
        F: Fn(ReplicationMessage) + Send + Sync + 'static,
    {
        let session = SessionContext::global(cx);
        session.start_session(our_peer_id.clone(), host_peer_id);
        session.set_message_sender(send_callback);

        tracing::info!(
            "Multiuser replication session started (peer: {})",
            our_peer_id
        );
    }

    /// End the multiuser session
    pub fn end_session(&self, cx: &App) {
        let session = SessionContext::global(cx);
        session.end_session();

        let registry = ReplicationRegistry::global(cx);
        registry.clear();

        tracing::info!("Multiuser replication session ended");
    }

    /// Handle an incoming replication message from the network.
    /// Returns `Some(message)` if a response should be sent back.
    pub fn handle_incoming_message(
        &self,
        message: ReplicationMessage,
        cx: &App,
    ) -> Option<ReplicationMessage> {
        let mut handler = ReplicationMessageHandler::new(cx);
        handler.handle_message(message)
    }

    /// Broadcast a replication message to all peers
    pub fn broadcast_message(&self, message: ReplicationMessage, cx: &App) {
        let session = SessionContext::global(cx);
        session.send_message(message);
    }

    /// Add a user to the session
    pub fn add_user(&self, peer_id: String, display_name: String, color: gpui::Hsla, cx: &App) {
        let presence = UserPresence::new(peer_id.clone(), display_name, color);
        let registry = ReplicationRegistry::global(cx);
        registry.update_user_presence(presence);

        tracing::debug!("Added user {} to replication session", peer_id);
    }

    /// Remove a user from the session
    pub fn remove_user(&self, peer_id: &str, cx: &App) {
        let registry = ReplicationRegistry::global(cx);
        registry.remove_user_presence(peer_id);

        tracing::debug!("Removed user {} from replication session", peer_id);
    }

    /// Update a user's presence information
    pub fn update_user_presence(&self, presence: UserPresence, cx: &App) {
        let registry = ReplicationRegistry::global(cx);
        registry.update_user_presence(presence);
    }

    /// Set a permission handler for RequestEdit mode
    pub fn set_permission_handler<F>(&self, handler: F, cx: &App)
    where
        F: Fn(&str, &str) -> bool + Send + Sync + 'static,
    {
        let session = SessionContext::global(cx);
        session.set_permission_handler(handler);
    }

    /// Get the current session context
    pub fn session_context(&self, cx: &App) -> SessionContext {
        SessionContext::global(cx)
    }

    /// Get the replication registry
    pub fn registry(&self, cx: &App) -> ReplicationRegistry {
        ReplicationRegistry::global(cx)
    }
}
