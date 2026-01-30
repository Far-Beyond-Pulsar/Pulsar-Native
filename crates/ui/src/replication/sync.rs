use super::{ReplicationRegistry, UserPresence};
use gpui::{App, AppContext};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Messages sent between clients for state replication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplicationMessage {
    /// Element state has changed
    StateUpdate {
        element_id: String,
        state: serde_json::Value,
        timestamp: u64,
        peer_id: String,
    },

    /// User started editing an element
    EditorJoined {
        element_id: String,
        peer_id: String,
    },

    /// User stopped editing an element
    EditorLeft {
        element_id: String,
        peer_id: String,
    },

    /// User entered a panel/tab
    PanelJoined {
        panel_id: String,
        peer_id: String,
    },

    /// User left a panel/tab
    PanelLeft {
        panel_id: String,
        peer_id: String,
    },

    /// User presence update (cursor position, selection, etc.)
    PresenceUpdate {
        peer_id: String,
        presence: UserPresence,
    },

    /// Request edit lock (for LockedEdit mode)
    RequestLock {
        element_id: String,
        peer_id: String,
    },

    /// Release edit lock (for LockedEdit mode)
    ReleaseLock {
        element_id: String,
        peer_id: String,
    },

    /// Grant edit lock to a user
    LockGranted {
        element_id: String,
        peer_id: String,
    },

    /// Deny edit lock request
    LockDenied {
        element_id: String,
        peer_id: String,
        reason: String,
    },

    /// Request edit permission (for RequestEdit mode)
    RequestPermission {
        element_id: String,
        peer_id: String,
    },

    /// Grant edit permission
    PermissionGranted {
        element_id: String,
        peer_id: String,
    },

    /// Deny edit permission
    PermissionDenied {
        element_id: String,
        peer_id: String,
        reason: String,
    },

    /// Request full state sync for an element
    RequestSync {
        element_id: String,
        peer_id: String,
    },

    /// Full state sync response
    SyncResponse {
        element_id: String,
        state: serde_json::Value,
        timestamp: u64,
    },
}

/// Handler for processing replication messages
pub struct ReplicationMessageHandler {
    registry: ReplicationRegistry,
}

impl ReplicationMessageHandler {
    pub fn new(cx: &App) -> Self {
        Self {
            registry: ReplicationRegistry::global(cx),
        }
    }

    /// Process an incoming replication message
    pub fn handle_message(&mut self, message: ReplicationMessage) -> Option<ReplicationMessage> {
        match message {
            ReplicationMessage::StateUpdate {
                element_id,
                state,
                timestamp,
                peer_id,
            } => {
                self.handle_state_update(&element_id, state, timestamp, &peer_id);
                None
            }

            ReplicationMessage::EditorJoined { element_id, peer_id } => {
                self.handle_editor_joined(&element_id, &peer_id);
                None
            }

            ReplicationMessage::EditorLeft { element_id, peer_id } => {
                self.handle_editor_left(&element_id, &peer_id);
                None
            }

            ReplicationMessage::PanelJoined { panel_id, peer_id } => {
                self.handle_panel_joined(&panel_id, &peer_id);
                None
            }

            ReplicationMessage::PanelLeft { panel_id, peer_id } => {
                self.handle_panel_left(&panel_id, &peer_id);
                None
            }

            ReplicationMessage::PresenceUpdate { peer_id, presence } => {
                self.handle_presence_update(&peer_id, presence);
                None
            }

            ReplicationMessage::RequestLock { element_id, peer_id } => {
                self.handle_lock_request(&element_id, &peer_id)
            }

            ReplicationMessage::ReleaseLock { element_id, peer_id } => {
                self.handle_lock_release(&element_id, &peer_id);
                None
            }

            ReplicationMessage::RequestPermission { element_id, peer_id } => {
                self.handle_permission_request(&element_id, &peer_id);
                None
            }

            ReplicationMessage::RequestSync { element_id, peer_id } => {
                self.handle_sync_request(&element_id, &peer_id)
            }

            // These are responses - typically handled by caller
            ReplicationMessage::LockGranted { .. }
            | ReplicationMessage::LockDenied { .. }
            | ReplicationMessage::PermissionGranted { .. }
            | ReplicationMessage::PermissionDenied { .. }
            | ReplicationMessage::SyncResponse { .. } => None,
        }
    }

    fn handle_state_update(
        &mut self,
        element_id: &str,
        state: serde_json::Value,
        timestamp: u64,
        peer_id: &str,
    ) {
        // Check if this is a newer update
        if let Some(elem_state) = self.registry.get_element_state(element_id) {
            if timestamp <= elem_state.last_update {
                tracing::debug!(
                    "Ignoring stale state update for {} (timestamp {} <= {})",
                    element_id,
                    timestamp,
                    elem_state.last_update
                );
                return;
            }

            // Check if user is allowed to send updates
            if !elem_state.can_edit(peer_id) {
                tracing::warn!(
                    "User {} tried to update element {} without permission",
                    peer_id,
                    element_id
                );
                return;
            }
        }

        // Apply the update
        self.registry
            .update_element_state(element_id, state, timestamp);
        tracing::debug!(
            "Applied state update for {} from user {}",
            element_id,
            peer_id
        );
    }

    fn handle_editor_joined(&mut self, element_id: &str, peer_id: &str) {
        if self.registry.add_editor(element_id, peer_id) {
            tracing::debug!("User {} joined editing {}", peer_id, element_id);
        } else {
            tracing::warn!(
                "User {} tried to join editing {} but was denied",
                peer_id,
                element_id
            );
        }
    }

    fn handle_editor_left(&mut self, element_id: &str, peer_id: &str) {
        self.registry.remove_editor(element_id, peer_id);
        tracing::debug!("User {} left editing {}", peer_id, element_id);
    }

    fn handle_panel_joined(&mut self, panel_id: &str, peer_id: &str) {
        self.registry.add_panel_presence(panel_id, peer_id);
        tracing::debug!("User {} joined panel {}", peer_id, panel_id);
    }

    fn handle_panel_left(&mut self, panel_id: &str, peer_id: &str) {
        self.registry.remove_panel_presence(panel_id, peer_id);
        tracing::debug!("User {} left panel {}", peer_id, panel_id);
    }

    fn handle_presence_update(&mut self, peer_id: &str, presence: UserPresence) {
        self.registry.update_user_presence(presence);
        tracing::debug!("Updated presence for user {}", peer_id);
    }

    fn handle_lock_request(&mut self, element_id: &str, peer_id: &str) -> Option<ReplicationMessage> {
        if let Some(mut elem_state) = self.registry.get_element_state(element_id) {
            if elem_state.acquire_lock(peer_id) {
                tracing::debug!(
                    "Granted lock on {} to user {}",
                    element_id,
                    peer_id
                );
                return Some(ReplicationMessage::LockGranted {
                    element_id: element_id.to_string(),
                    peer_id: peer_id.to_string(),
                });
            } else {
                let holder = elem_state.locked_by.as_deref().unwrap_or("unknown");
                tracing::debug!(
                    "Denied lock on {} to user {} (held by {})",
                    element_id,
                    peer_id,
                    holder
                );
                return Some(ReplicationMessage::LockDenied {
                    element_id: element_id.to_string(),
                    peer_id: peer_id.to_string(),
                    reason: format!("Locked by {}", holder),
                });
            }
        }

        None
    }

    fn handle_lock_release(&mut self, element_id: &str, peer_id: &str) {
        if let Some(mut elem_state) = self.registry.get_element_state(element_id) {
            elem_state.release_lock(peer_id);
            tracing::debug!("Released lock on {} by user {}", element_id, peer_id);
        }
    }

    fn handle_permission_request(&mut self, element_id: &str, peer_id: &str) {
        if let Some(mut elem_state) = self.registry.get_element_state(element_id) {
            elem_state.request_permission(peer_id);
            tracing::debug!(
                "User {} requested permission for {}",
                peer_id,
                element_id
            );

            // Emit notification for UI to handle
            // The application layer should subscribe to state changes
            // and show a notification to the host/admin
            tracing::info!(
                "Permission request pending for element {} from user {}",
                element_id,
                peer_id
            );

            // The registry's on_state_change callback can be used to notify the UI
            // See SessionContext::set_permission_handler for the handler
        }
    }

    fn handle_sync_request(&mut self, element_id: &str, _peer_id: &str) -> Option<ReplicationMessage> {
        if let Some(elem_state) = self.registry.get_element_state(element_id) {
            if let Some(state) = elem_state.last_state {
                return Some(ReplicationMessage::SyncResponse {
                    element_id: element_id.to_string(),
                    state,
                    timestamp: elem_state.last_update,
                });
            }
        }

        None
    }
}

/// Helper to create replication messages
pub struct ReplicationMessageBuilder;

impl ReplicationMessageBuilder {
    /// Create a state update message
    pub fn state_update(
        element_id: impl Into<String>,
        state: serde_json::Value,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::StateUpdate {
            element_id: element_id.into(),
            state,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            peer_id: peer_id.into(),
        }
    }

    /// Create an editor joined message
    pub fn editor_joined(
        element_id: impl Into<String>,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::EditorJoined {
            element_id: element_id.into(),
            peer_id: peer_id.into(),
        }
    }

    /// Create an editor left message
    pub fn editor_left(
        element_id: impl Into<String>,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::EditorLeft {
            element_id: element_id.into(),
            peer_id: peer_id.into(),
        }
    }

    /// Create a panel joined message
    pub fn panel_joined(
        panel_id: impl Into<String>,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::PanelJoined {
            panel_id: panel_id.into(),
            peer_id: peer_id.into(),
        }
    }

    /// Create a panel left message
    pub fn panel_left(
        panel_id: impl Into<String>,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::PanelLeft {
            panel_id: panel_id.into(),
            peer_id: peer_id.into(),
        }
    }

    /// Create a presence update message
    pub fn presence_update(presence: UserPresence) -> ReplicationMessage {
        let peer_id = presence.peer_id.clone();
        ReplicationMessage::PresenceUpdate { peer_id, presence }
    }

    /// Create a lock request message
    pub fn request_lock(
        element_id: impl Into<String>,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::RequestLock {
            element_id: element_id.into(),
            peer_id: peer_id.into(),
        }
    }

    /// Create a lock release message
    pub fn release_lock(
        element_id: impl Into<String>,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::ReleaseLock {
            element_id: element_id.into(),
            peer_id: peer_id.into(),
        }
    }

    /// Create a permission request message
    pub fn request_permission(
        element_id: impl Into<String>,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::RequestPermission {
            element_id: element_id.into(),
            peer_id: peer_id.into(),
        }
    }

    /// Create a sync request message
    pub fn request_sync(
        element_id: impl Into<String>,
        peer_id: impl Into<String>,
    ) -> ReplicationMessage {
        ReplicationMessage::RequestSync {
            element_id: element_id.into(),
            peer_id: peer_id.into(),
        }
    }
}
