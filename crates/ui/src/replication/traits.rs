use super::{ReplicationConfig, ReplicationMode};
use gpui::{App, Context, Entity, Window};
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

        // Check if we should accept updates based on mode
        match config.mode {
            ReplicationMode::NoRep => {
                // Never accept remote updates in NoRep mode
                return Ok(false);
            }
            ReplicationMode::BroadcastOnly => {
                // Only accept from host (peer_id "0" or first peer)
                // TODO: Get actual host ID from session
                if peer_id != "0" {
                    return Ok(false);
                }
            }
            ReplicationMode::LockedEdit => {
                // Only accept if we're not currently editing
                // TODO: Check lock state
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
    fn request_edit_permission(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        let config = self.replication_config();
        if config.mode != ReplicationMode::RequestEdit {
            return true; // Not in request mode, allow immediately
        }

        // TODO: Send permission request to host
        // For now, auto-grant
        tracing::warn!("RequestEdit permission not implemented, auto-granting");
        true
    }

    /// Check if the current user can edit this element
    fn can_edit(&self, _cx: &App) -> bool {
        let config = self.replication_config();

        match config.mode {
            ReplicationMode::NoRep => true,
            ReplicationMode::MultiEdit => true,
            ReplicationMode::LockedEdit => {
                // TODO: Check if we hold the lock
                true
            }
            ReplicationMode::RequestEdit => {
                // TODO: Check if we have permission
                true
            }
            ReplicationMode::BroadcastOnly => {
                // TODO: Check if we're the host
                false
            }
            ReplicationMode::Follow => {
                // TODO: Check if we're not in follow mode
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
        self.update(cx, |this, _cx| {
            this.set_replication_mode(mode);
        });
        self
    }

    fn sync_state(&self, cx: &mut App) {
        let (id, state) = self.read(cx).replication_id().clone()
            .and_then(|id| {
                self.read(cx)
                    .serialize_state(cx)
                    .ok()
                    .map(|state| (id, state))
            })
            .unwrap_or_else(|| {
                tracing::warn!("Failed to serialize replicated state");
                return;
            });

        // TODO: Send state update via multiuser client
        tracing::debug!("Syncing state for element {}: {:?}", id, state);
    }

    fn subscribe_to_replication(&self, cx: &mut App) {
        // TODO: Subscribe to multiuser events for this element
        let id = self.read(cx).replication_id();
        tracing::debug!("Subscribed to replication for element {}", id);
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
