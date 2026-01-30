use super::{ReplicationConfig, ReplicationMode, UserPresence};
use gpui::{App, AppContext, Global};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Global registry of all replicated elements in the application
pub struct ReplicationRegistry {
    /// Map of element_id -> replication config
    elements: Arc<RwLock<HashMap<String, ElementReplicationState>>>,

    /// Map of panel_id -> list of active user peer_ids
    panel_presences: Arc<RwLock<HashMap<String, Vec<String>>>>,

    /// Map of peer_id -> UserPresence
    user_presences: Arc<RwLock<HashMap<String, UserPresence>>>,

    /// Callback invoked when an element's state changes
    on_state_change: Arc<RwLock<Option<Box<dyn Fn(&str, &serde_json::Value) + Send + Sync>>>>,
}

impl Global for ReplicationRegistry {}

/// State tracking for a single replicated element
#[derive(Debug, Clone)]
pub struct ElementReplicationState {
    /// Unique identifier for the element
    pub element_id: String,

    /// Replication configuration
    pub config: ReplicationConfig,

    /// Last known state (serialized as JSON)
    pub last_state: Option<serde_json::Value>,

    /// Timestamp of last state update
    pub last_update: u64,

    /// List of peer IDs currently editing this element
    pub active_editors: Vec<String>,

    /// Current lock holder (for LockedEdit mode)
    pub locked_by: Option<String>,

    /// Pending edit requests (for RequestEdit mode)
    pub pending_requests: Vec<String>,
}

impl ElementReplicationState {
    pub fn new(element_id: String, config: ReplicationConfig) -> Self {
        Self {
            element_id,
            config,
            last_state: None,
            last_update: 0,
            active_editors: Vec::new(),
            locked_by: None,
            pending_requests: Vec::new(),
        }
    }

    /// Check if a user can edit this element
    pub fn can_edit(&self, peer_id: &str) -> bool {
        match self.config.mode {
            ReplicationMode::NoRep => true,
            ReplicationMode::MultiEdit => {
                // Check max concurrent editors if set
                if let Some(max) = self.config.max_concurrent_editors {
                    if self.active_editors.len() >= max && !self.active_editors.contains(&peer_id.to_string()) {
                        return false;
                    }
                }
                true
            }
            ReplicationMode::LockedEdit => {
                // Can edit if we hold the lock or no one does
                self.locked_by.is_none() || self.locked_by.as_deref() == Some(peer_id)
            }
            ReplicationMode::RequestEdit => {
                // Can edit if request was approved
                self.active_editors.contains(&peer_id.to_string())
            }
            ReplicationMode::BroadcastOnly => {
                // Only host can edit (peer_id "0")
                peer_id == "0"
            }
            ReplicationMode::Follow => true,
            ReplicationMode::QueuedEdit => true,
            ReplicationMode::PartitionedEdit => true,
        }
    }

    /// Acquire edit lock (for LockedEdit mode)
    pub fn acquire_lock(&mut self, peer_id: &str) -> bool {
        if self.config.mode != ReplicationMode::LockedEdit {
            return true;
        }

        if self.locked_by.is_none() {
            self.locked_by = Some(peer_id.to_string());
            true
        } else {
            false
        }
    }

    /// Release edit lock (for LockedEdit mode)
    pub fn release_lock(&mut self, peer_id: &str) {
        if self.locked_by.as_deref() == Some(peer_id) {
            self.locked_by = None;
        }
    }

    /// Request edit permission (for RequestEdit mode)
    pub fn request_permission(&mut self, peer_id: &str) {
        if self.config.mode != ReplicationMode::RequestEdit {
            return;
        }

        if !self.pending_requests.contains(&peer_id.to_string()) {
            self.pending_requests.push(peer_id.to_string());
        }
    }

    /// Grant edit permission (for RequestEdit mode)
    pub fn grant_permission(&mut self, peer_id: &str) {
        self.pending_requests.retain(|id| id != peer_id);
        if !self.active_editors.contains(&peer_id.to_string()) {
            self.active_editors.push(peer_id.to_string());
        }
    }

    /// Revoke edit permission (for RequestEdit mode)
    pub fn revoke_permission(&mut self, peer_id: &str) {
        self.active_editors.retain(|id| id != peer_id);
    }
}

impl ReplicationRegistry {
    /// Initialize the global registry
    pub fn init(cx: &mut App) {
        cx.set_global(Self {
            elements: Arc::new(RwLock::new(HashMap::new())),
            panel_presences: Arc::new(RwLock::new(HashMap::new())),
            user_presences: Arc::new(RwLock::new(HashMap::new())),
            on_state_change: Arc::new(RwLock::new(None)),
        });
    }

    /// Get the global registry instance
    pub fn global(cx: &App) -> Self {
        cx.global::<Self>().clone()
    }

    /// Register a new replicated element
    pub fn register_element(&self, element_id: String, config: ReplicationConfig) {
        let state = ElementReplicationState::new(element_id.clone(), config);
        let mut elements = self.elements.write().unwrap();
        elements.insert(element_id, state);
    }

    /// Unregister a replicated element
    pub fn unregister_element(&self, element_id: &str) {
        let mut elements = self.elements.write().unwrap();
        elements.remove(element_id);
    }

    /// Get the state of a replicated element
    pub fn get_element_state(&self, element_id: &str) -> Option<ElementReplicationState> {
        let elements = self.elements.read().unwrap();
        elements.get(element_id).cloned()
    }

    /// Update the state of a replicated element
    pub fn update_element_state(
        &self,
        element_id: &str,
        state: serde_json::Value,
        timestamp: u64,
    ) -> bool {
        let mut elements = self.elements.write().unwrap();
        if let Some(elem_state) = elements.get_mut(element_id) {
            elem_state.last_state = Some(state.clone());
            elem_state.last_update = timestamp;

            // Notify listeners
            if let Some(callback) = self.on_state_change.read().unwrap().as_ref() {
                callback(element_id, &state);
            }

            true
        } else {
            false
        }
    }

    /// Add a user as an active editor for an element
    pub fn add_editor(&self, element_id: &str, peer_id: &str) -> bool {
        let mut elements = self.elements.write().unwrap();
        if let Some(state) = elements.get_mut(element_id) {
            if state.can_edit(peer_id) && !state.active_editors.contains(&peer_id.to_string()) {
                state.active_editors.push(peer_id.to_string());
                return true;
            }
        }
        false
    }

    /// Remove a user as an active editor for an element
    pub fn remove_editor(&self, element_id: &str, peer_id: &str) {
        let mut elements = self.elements.write().unwrap();
        if let Some(state) = elements.get_mut(element_id) {
            state.active_editors.retain(|id| id != peer_id);
            state.release_lock(peer_id);
        }
    }

    /// Get all active editors for an element
    pub fn get_editors(&self, element_id: &str) -> Vec<String> {
        let elements = self.elements.read().unwrap();
        elements
            .get(element_id)
            .map(|state| state.active_editors.clone())
            .unwrap_or_default()
    }

    /// Add a user presence to a panel
    pub fn add_panel_presence(&self, panel_id: &str, peer_id: &str) {
        let mut presences = self.panel_presences.write().unwrap();
        presences
            .entry(panel_id.to_string())
            .or_insert_with(Vec::new)
            .push(peer_id.to_string());
    }

    /// Remove a user presence from a panel
    pub fn remove_panel_presence(&self, panel_id: &str, peer_id: &str) {
        let mut presences = self.panel_presences.write().unwrap();
        if let Some(users) = presences.get_mut(panel_id) {
            users.retain(|id| id != peer_id);
            if users.is_empty() {
                presences.remove(panel_id);
            }
        }
    }

    /// Get all users present in a panel
    pub fn get_panel_users(&self, panel_id: &str) -> Vec<String> {
        let presences = self.panel_presences.read().unwrap();
        presences.get(panel_id).cloned().unwrap_or_default()
    }

    /// Update or add a user presence
    pub fn update_user_presence(&self, presence: UserPresence) {
        let mut presences = self.user_presences.write().unwrap();
        presences.insert(presence.peer_id.clone(), presence);
    }

    /// Get a user's presence
    pub fn get_user_presence(&self, peer_id: &str) -> Option<UserPresence> {
        let presences = self.user_presences.read().unwrap();
        presences.get(peer_id).cloned()
    }

    /// Get all user presences
    pub fn get_all_presences(&self) -> Vec<UserPresence> {
        let presences = self.user_presences.read().unwrap();
        presences.values().cloned().collect()
    }

    /// Remove a user presence
    pub fn remove_user_presence(&self, peer_id: &str) {
        let mut presences = self.user_presences.write().unwrap();
        presences.remove(peer_id);

        // Also remove from all panels
        let mut panel_presences = self.panel_presences.write().unwrap();
        for users in panel_presences.values_mut() {
            users.retain(|id| id != peer_id);
        }
        panel_presences.retain(|_, users| !users.is_empty());

        // Remove from all element editors
        let mut elements = self.elements.write().unwrap();
        for state in elements.values_mut() {
            state.active_editors.retain(|id| id != peer_id);
            if state.locked_by.as_deref() == Some(peer_id) {
                state.locked_by = None;
            }
            state.pending_requests.retain(|id| id != peer_id);
        }
    }

    /// Set callback for state changes
    pub fn on_state_change<F>(&self, callback: F)
    where
        F: Fn(&str, &serde_json::Value) + Send + Sync + 'static,
    {
        let mut on_change = self.on_state_change.write().unwrap();
        *on_change = Some(Box::new(callback));
    }

    /// Clear all state (useful for tests or session end)
    pub fn clear(&self) {
        let mut elements = self.elements.write().unwrap();
        elements.clear();

        let mut panel_presences = self.panel_presences.write().unwrap();
        panel_presences.clear();

        let mut user_presences = self.user_presences.write().unwrap();
        user_presences.clear();
    }
}

impl Clone for ReplicationRegistry {
    fn clone(&self) -> Self {
        Self {
            elements: Arc::clone(&self.elements),
            panel_presences: Arc::clone(&self.panel_presences),
            user_presences: Arc::clone(&self.user_presences),
            on_state_change: Arc::clone(&self.on_state_change),
        }
    }
}
