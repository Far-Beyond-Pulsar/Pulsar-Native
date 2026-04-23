use crate::{
    ReplicationConfig, ReplicationMessageBuilder, ReplicationMode, ReplicationRegistry,
    SessionContext,
};
use gpui::{App, Entity, Window};
use serde_json::Value;

/// Trait for components that can replicate their state across users
///
/// Implement this trait to make your UI component network-aware and
/// enable multi-user collaboration.
pub trait Replicator: Sized {
    /// Returns the unique identifier for this replicated element
    fn replication_id(&self) -> String;

    /// Returns the current replication configuration
    fn replication_config(&self) -> &ReplicationConfig;

    /// Returns a mutable reference to the replication configuration
    fn replication_config_mut(&mut self) -> &mut ReplicationConfig;

    /// Set the replication mode for this element
    fn set_replication_mode(&mut self, mode: ReplicationMode) {
        self.replication_config_mut().mode = mode;
    }

    /// Serialize the current state for network transmission
    fn serialize_state(&self, cx: &App) -> Result<Value, String>;

    /// Deserialize and apply state received from the network
    fn deserialize_state(
        &mut self,
        state: Value,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String>;

    /// Called when another user starts editing this element
    fn on_remote_user_joined(&mut self, peer_id: &str, _window: &mut Window, _cx: &mut App) {
        tracing::debug!(
            "User {} started editing element {}",
            peer_id,
            self.replication_id()
        );
    }

    /// Called when another user stops editing this element
    fn on_remote_user_left(&mut self, peer_id: &str, _window: &mut Window, _cx: &mut App) {
        tracing::debug!(
            "User {} stopped editing element {}",
            peer_id,
            self.replication_id()
        );
    }

    /// Called when this element receives a remote state update.
    /// Return `Ok(false)` to reject the update.
    fn on_remote_state_update(
        &mut self,
        peer_id: &str,
        state: Value,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<bool, String> {
        let config = self.replication_config();
        let session = SessionContext::global(cx);
        let registry = ReplicationRegistry::global(cx);
        let element_id = self.replication_id();

        match config.mode {
            ReplicationMode::NoRep => {
                return Ok(false);
            }
            ReplicationMode::BroadcastOnly => {
                if let Some(host_id) = session.host_peer_id() {
                    if peer_id != host_id {
                        tracing::debug!(
                            "Rejecting update from {} (not host) for element {}",
                            peer_id,
                            element_id
                        );
                        return Ok(false);
                    }
                } else {
                    return Ok(false);
                }
            }
            ReplicationMode::LockedEdit => {
                if let Some(elem_state) = registry.get_element_state(&element_id) {
                    if let Some(lock_holder) = &elem_state.locked_by {
                        if lock_holder != peer_id {
                            tracing::debug!(
                                "Rejecting update from {} for locked element {} (held by {})",
                                peer_id,
                                element_id,
                                lock_holder
                            );
                            return Ok(false);
                        }
                    }
                }
            }
            ReplicationMode::RequestEdit => {
                if let Some(elem_state) = registry.get_element_state(&element_id) {
                    if !elem_state.active_editors.contains(&peer_id.to_string()) {
                        tracing::debug!(
                            "Rejecting update from unapproved user {} for element {}",
                            peer_id,
                            element_id
                        );
                        return Ok(false);
                    }
                }
            }
            _ => {}
        }

        self.deserialize_state(state, window, cx)?;
        Ok(true)
    }

    /// Request edit permission (for RequestEdit mode). Returns true if granted immediately.
    fn request_edit_permission(&mut self, _window: &mut Window, cx: &mut App) -> bool {
        let config = self.replication_config();
        if config.mode != ReplicationMode::RequestEdit {
            return true;
        }

        let session = SessionContext::global(cx);
        let registry = ReplicationRegistry::global(cx);
        let element_id = self.replication_id();

        if session.are_we_host() {
            return session.request_permission(&element_id);
        }

        if let Some(our_peer_id) = session.our_peer_id() {
            let message = ReplicationMessageBuilder::request_permission(&element_id, &our_peer_id);
            session.send_message(message);

            if let Some(mut elem_state) = registry.get_element_state(&element_id) {
                elem_state.request_permission(&our_peer_id);
            }

            tracing::debug!("Sent permission request for element {}", element_id);
        }

        false
    }

    /// Check if the current user can edit this element
    fn can_edit(&self, cx: &App) -> bool {
        let config = self.replication_config();
        let session = SessionContext::global(cx);
        let registry = ReplicationRegistry::global(cx);
        let element_id = self.replication_id();

        if !session.is_active() {
            return true;
        }

        let our_peer_id = match session.our_peer_id() {
            Some(id) => id,
            None => return false,
        };

        match config.mode {
            ReplicationMode::NoRep => true,
            ReplicationMode::MultiEdit => {
                if let Some(elem_state) = registry.get_element_state(&element_id) {
                    if let Some(max) = config.max_concurrent_editors {
                        if elem_state.active_editors.len() >= max
                            && !elem_state.active_editors.contains(&our_peer_id)
                        {
                            return false;
                        }
                    }
                }
                true
            }
            ReplicationMode::LockedEdit => {
                if let Some(elem_state) = registry.get_element_state(&element_id) {
                    elem_state.locked_by.is_none()
                        || elem_state.locked_by.as_ref() == Some(&our_peer_id)
                } else {
                    true
                }
            }
            ReplicationMode::RequestEdit => {
                if let Some(elem_state) = registry.get_element_state(&element_id) {
                    elem_state.active_editors.contains(&our_peer_id)
                } else {
                    false
                }
            }
            ReplicationMode::BroadcastOnly => session.are_we_host(),
            ReplicationMode::Follow => true,
            ReplicationMode::QueuedEdit => true,
            ReplicationMode::PartitionedEdit => true,
        }
    }
}

/// Extension trait for `Entity<T>` where `T: Replicator`
pub trait ReplicatorExt<T: Replicator> {
    /// Configure replication for this entity
    fn with_replication(self, mode: ReplicationMode, cx: &mut App) -> Self;

    /// Sync state to all connected users
    fn sync_state(&self, cx: &mut App);

    /// Subscribe to remote state changes
    fn subscribe_to_replication(&self, cx: &mut App);
}

impl<T: Replicator + 'static> ReplicatorExt<T> for Entity<T> {
    fn with_replication(self, mode: ReplicationMode, cx: &mut App) -> Self {
        let element_id = self.read(cx).replication_id();
        let config = self.read(cx).replication_config().clone();

        self.update(cx, |this, _cx| {
            this.set_replication_mode(mode);
        });

        let registry = ReplicationRegistry::global(cx);
        registry.register_element(element_id, config);

        self
    }

    fn sync_state(&self, cx: &mut App) {
        let session = SessionContext::global(cx);
        let registry = ReplicationRegistry::global(cx);

        let element_id = self.read(cx).replication_id();
        let state = match self.read(cx).serialize_state(cx) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to serialize state for {}: {}", element_id, e);
                return;
            }
        };

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        registry.update_element_state(&element_id, state.clone(), timestamp);

        if session.is_active() {
            if let Some(our_peer_id) = session.our_peer_id() {
                let message =
                    ReplicationMessageBuilder::state_update(element_id.clone(), state, our_peer_id);
                session.send_message(message);

                tracing::debug!("Synced state for element {}", element_id);
            }
        }
    }

    fn subscribe_to_replication(&self, cx: &mut App) {
        let element_id = self.read(cx).replication_id();
        let config = self.read(cx).replication_config().clone();

        let registry = ReplicationRegistry::global(cx);
        registry.register_element(element_id.clone(), config);

        tracing::debug!("Subscribed to replication for element {}", element_id);
    }
}

/// Trait for panels/tabs that can show user presence
pub trait PresenceAware {
    /// Returns the list of users currently active in this panel/tab
    fn active_users(&self) -> Vec<String>;

    /// Add a user to this panel/tab's presence
    fn add_user_presence(&mut self, peer_id: String);

    /// Remove a user from this panel/tab's presence
    fn remove_user_presence(&mut self, peer_id: &str);

    /// Check if a specific user is active in this panel/tab
    fn has_user(&self, peer_id: &str) -> bool {
        self.active_users().iter().any(|id| id == peer_id)
    }

    /// Get the count of active users
    fn user_count(&self) -> usize {
        self.active_users().len()
    }
}
