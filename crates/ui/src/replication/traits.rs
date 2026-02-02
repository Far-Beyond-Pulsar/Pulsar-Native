use super::{ReplicationConfig, ReplicationMode, ReplicationMessageBuilder, ReplicationRegistry, SessionContext};
use gpui::{App, AppContext, Context, Entity, Window};
use serde_json::Value;

/// Trait for components that can replicate their state across users
///
/// Implement this trait to make your UI component network-aware and
/// enable multi-user collaboration.
pub trait Replicator: Sized {
    /// Returns the unique identifier for this replicated element
    ///
    /// This ID is used to identify the element across the network.
    /// It should be stable across sessions if persistence is desired.
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
    ///
    /// This should capture all relevant state that needs to be synchronized.
    /// The returned JSON should be deterministic and minimal.
    fn serialize_state(&self, cx: &App) -> Result<Value, String>;

    /// Deserialize and apply state received from the network
    ///
    /// This should update the component's state to match the received data.
    /// Return false if the update should be rejected.
    fn deserialize_state(&mut self, state: Value, window: &mut Window, cx: &mut App) -> Result<(), String>;

    /// Called when another user starts editing this element
    ///
    /// Use this to show presence indicators, disable editing (for locked mode), etc.
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

    /// Called when this element receives a remote state update
    ///
    /// Return false to reject the update (e.g., if locked by another user)
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

        // Check if we should accept updates based on mode
        match config.mode {
            ReplicationMode::NoRep => {
                // Never accept remote updates in NoRep mode
                return Ok(false);
            }
            ReplicationMode::BroadcastOnly => {
                // Only accept from host
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
                    // No host set, reject
                    return Ok(false);
                }
            }
            ReplicationMode::LockedEdit => {
                // Check if element is locked and who holds it
                if let Some(elem_state) = registry.get_element_state(&element_id) {
                    if let Some(lock_holder) = &elem_state.locked_by {
                        // Only accept updates from the lock holder
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
                // Only accept updates from approved editors
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
            _ => {
                // MultiEdit, Follow, Queued, Partitioned - accept updates
            }
        }

        // Apply the update
        self.deserialize_state(state, window, cx)?;
        Ok(true)
    }

    /// Request edit permission (for RequestEdit mode)
    ///
    /// Returns true if permission was granted immediately, false if pending
    fn request_edit_permission(&mut self, _window: &mut Window, cx: &mut App) -> bool {
        let config = self.replication_config();
        if config.mode != ReplicationMode::RequestEdit {
            return true; // Not in request mode, allow immediately
        }

        let session = SessionContext::global(cx);
        let registry = ReplicationRegistry::global(cx);
        let element_id = self.replication_id();

        // If we're the host, check with permission handler
        if session.are_we_host() {
            return session.request_permission(&element_id);
        }

        // Send permission request to host
        if let Some(our_peer_id) = session.our_peer_id() {
            let message = ReplicationMessageBuilder::request_permission(&element_id, &our_peer_id);
            session.send_message(message);

            // Mark as pending in registry
            if let Some(mut elem_state) = registry.get_element_state(&element_id) {
                elem_state.request_permission(&our_peer_id);
            }

            tracing::debug!("Sent permission request for element {}", element_id);
        }

        false // Permission pending
    }

    /// Check if the current user can edit this element
    fn can_edit(&self, cx: &App) -> bool {
        let config = self.replication_config();
        let session = SessionContext::global(cx);
        let registry = ReplicationRegistry::global(cx);
        let element_id = self.replication_id();

        // If not in a session, allow local editing
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
                // Check max concurrent editors if set
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
                // Check if we hold the lock or no one does
                if let Some(elem_state) = registry.get_element_state(&element_id) {
                    elem_state.locked_by.is_none()
                        || elem_state.locked_by.as_ref() == Some(&our_peer_id)
                } else {
                    true // No state yet, allow
                }
            }
            ReplicationMode::RequestEdit => {
                // Check if we have permission
                if let Some(elem_state) = registry.get_element_state(&element_id) {
                    elem_state.active_editors.contains(&our_peer_id)
                } else {
                    // No state yet, request permission
                    false
                }
            }
            ReplicationMode::BroadcastOnly => {
                // Only host can edit
                session.are_we_host()
            }
            ReplicationMode::Follow => {
                // Can edit unless actively following someone
                // (following state would be tracked separately)
                true
            }
            ReplicationMode::QueuedEdit => true,
            ReplicationMode::PartitionedEdit => true,
        }
    }
}

/// Extension trait for Entity<T> where T: Replicator
///
/// This provides convenient methods for working with replicated entities.
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

        // Register with the registry
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

        // Update local registry
        registry.update_element_state(&element_id, state.clone(), timestamp);

        // Send to network if in a session
        if session.is_active() {
            if let Some(our_peer_id) = session.our_peer_id() {
                let message = ReplicationMessageBuilder::state_update(
                    element_id.clone(),
                    state,
                    our_peer_id,
                );
                session.send_message(message);

                tracing::debug!("Synced state for element {}", element_id);
            }
        }
    }

    fn subscribe_to_replication(&self, cx: &mut App) {
        let element_id = self.read(cx).replication_id();
        let config = self.read(cx).replication_config().clone();

        // Register with the registry
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
