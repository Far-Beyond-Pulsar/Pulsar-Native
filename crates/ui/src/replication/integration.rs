//! Integration layer for connecting replication to the multiuser client
//!
//! This module provides the glue code to wire up the replication system
//! with the existing multiuser networking infrastructure.

use super::{
    ReplicationMessage, ReplicationMessageBuilder, ReplicationMessageHandler, ReplicationRegistry,
    SessionContext, UserPresence,
};
use gpui::{App, AppContext};
use std::sync::Arc;

/// Helper to integrate replication with your multiuser client
///
/// This provides the connection between UI state replication and network transport.
///
/// # Usage
///
/// ```ignore
/// use ui::replication::MultiuserIntegration;
///
/// // During app initialization:
/// let integration = MultiuserIntegration::new(cx);
///
/// // When joining a session:
/// integration.start_session(
///     "our_peer_id".to_string(),
///     "host_peer_id".to_string(),
///     multiuser_client.clone(),
///     cx,
/// );
///
/// // The integration will automatically handle:
/// // - Sending replication messages via the client
/// // - Processing incoming replication messages
/// // - Managing user presence
/// ```
pub struct MultiuserIntegration;

impl MultiuserIntegration {
    /// Create a new integration instance
    pub fn new(cx: &App) -> Self {
        // Initialize session context if not already done
        if cx.try_global::<SessionContext>().is_none() {
            panic!("SessionContext not initialized. Call ui::init(cx) first.");
        }

        Self
    }

    /// Start a multiuser session with replication support
    ///
    /// This sets up the session context and wires up message handlers.
    ///
    /// # Arguments
    ///
    /// * `our_peer_id` - Our unique identifier in the session
    /// * `host_peer_id` - The session host's identifier
    /// * `send_callback` - Function to send messages to the network
    /// * `cx` - Application context
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

        // Start the session
        session.start_session(our_peer_id.clone(), host_peer_id);

        // Set up the message sender
        session.set_message_sender(send_callback);

        tracing::info!("Multiuser replication session started (peer: {})", our_peer_id);
    }

    /// End the multiuser session
    pub fn end_session(&self, cx: &App) {
        let session = SessionContext::global(cx);
        session.end_session();

        // Clear all replication state
        let registry = ReplicationRegistry::global(cx);
        registry.clear();

        tracing::info!("Multiuser replication session ended");
    }

    /// Handle an incoming replication message from the network
    ///
    /// Call this when you receive a replication message from another peer.
    ///
    /// Returns Some(message) if a response should be sent back.
    pub fn handle_incoming_message(
        &self,
        message: ReplicationMessage,
        cx: &App,
    ) -> Option<ReplicationMessage> {
        let mut handler = ReplicationMessageHandler::new(cx);
        handler.handle_message(message)
    }

    /// Broadcast a replication message to all peers
    ///
    /// This serializes the message and sends it via the configured sender.
    pub fn broadcast_message(&self, message: ReplicationMessage, cx: &App) {
        let session = SessionContext::global(cx);
        session.send_message(message);
    }

    /// Add a user to the session
    ///
    /// This creates a UserPresence and adds them to the registry.
    pub fn add_user(&self, peer_id: String, display_name: String, color: gpui::Hsla, cx: &App) {
        let presence = UserPresence::new(peer_id.clone(), display_name, color);

        let registry = ReplicationRegistry::global(cx);
        registry.update_user_presence(presence);

        tracing::debug!("Added user {} to replication session", peer_id);
    }

    /// Remove a user from the session
    ///
    /// This cleans up all their presence and editing state.
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
    ///
    /// The handler receives (element_id, peer_id) and returns whether to grant permission.
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

// Example integration with the existing multiuser client
//
// This shows how to wire up the replication system with your
// actual multiuser client implementation.
//
// ```ignore
// use engine_backend::subsystems::networking::multiuser::{MultiuserClient, ClientMessage, ServerMessage};
// use ui::replication::{MultiuserIntegration, ReplicationMessage};
// use std::sync::Arc;
// use tokio::sync::RwLock;
//
// pub struct ReplicationBridge {
//     integration: MultiuserIntegration,
//     client: Arc<RwLock<MultiuserClient>>,
// }
//
// impl ReplicationBridge {
//     pub fn new(client: Arc<RwLock<MultiuserClient>>, cx: &App) -> Self {
//         let integration = MultiuserIntegration::new(cx);
//
//         // Set up message sender
//         let client_clone = client.clone();
//         integration.start_session(
//             "our_peer_id".to_string(),
//             "host_peer_id".to_string(),
//             move |message: ReplicationMessage| {
//                 let client = client_clone.clone();
//                 tokio::spawn(async move {
//                     let data = serde_json::to_string(&message).unwrap();
//                     let client_guard = client.write().await;
//                     let _ = client_guard.send(ClientMessage::ReplicationUpdate { data }).await;
//                 });
//             },
//             cx,
//         );
//
//         Self { integration, client }
//     }
//
//     /// Process incoming server messages
//     pub async fn handle_server_message(&self, message: ServerMessage, cx: &App) {
//         match message {
//             ServerMessage::ReplicationUpdate { data } => {
//                 if let Ok(rep_msg) = serde_json::from_str::<ReplicationMessage>(&data) {
//                     if let Some(response) = self.integration.handle_incoming_message(rep_msg, cx) {
//                         // Send response back
//                         let data = serde_json::to_string(&response).unwrap();
//                         let client = self.client.write().await;
//                         let _ = client.send(ClientMessage::ReplicationUpdate { data }).await;
//                     }
//                 }
//             }
//             ServerMessage::PeerJoined { peer_id, .. } => {
//                 // Generate color for user
//                 let color = generate_user_color(&peer_id);
//                 self.integration.add_user(
//                     peer_id.clone(),
//                     format!("User {}", &peer_id[..8]),
//                     color,
//                     cx,
//                 );
//             }
//             ServerMessage::PeerLeft { peer_id } => {
//                 self.integration.remove_user(&peer_id, cx);
//             }
//             _ => {}
//         }
//     }
//
//     /// Notify when we enter a panel/tab
//     pub fn enter_panel(&self, panel_id: String, cx: &App) {
//         let session = self.integration.session_context(cx);
//         if let Some(our_peer_id) = session.our_peer_id() {
//             let message = ReplicationMessageBuilder::panel_joined(panel_id.clone(), our_peer_id.clone());
//             self.integration.broadcast_message(message, cx);
//
//             // Update local registry
//             let registry = self.integration.registry(cx);
//             registry.add_panel_presence(&panel_id, &our_peer_id);
//         }
//     }
//
//     /// Notify when we leave a panel/tab
//     pub fn leave_panel(&self, panel_id: String, cx: &App) {
//         let session = self.integration.session_context(cx);
//         if let Some(our_peer_id) = session.our_peer_id() {
//             let message = ReplicationMessageBuilder::panel_left(panel_id.clone(), our_peer_id.clone());
//             self.integration.broadcast_message(message, cx);
//
//             // Update local registry
//             let registry = self.integration.registry(cx);
//             registry.remove_panel_presence(&panel_id, &our_peer_id);
//         }
//     }
// }
//
// /// Generate a consistent color for a user based on their peer ID
// fn generate_user_color(peer_id: &str) -> gpui::Hsla {
//     use std::collections::hash_map::DefaultHasher;
//     use std::hash::{Hash, Hasher};
//
//     let mut hasher = DefaultHasher::new();
//     peer_id.hash(&mut hasher);
//     let hash = hasher.finish();
//
//     // Generate a vibrant color from the hash
//     let hue = (hash % 360) as f32;
//     let saturation = 0.7;
//     let lightness = 0.6;
//     let alpha = 1.0;
//
//     gpui::hsla(hue / 360.0, saturation, lightness, alpha)
// }
// ```