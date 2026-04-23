use crate::{ReplicationConfig, ReplicationMode, UserPresence};
use gpui::{App, Global};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

struct RegistryInner {
    elements: HashMap<String, ElementReplicationState>,
    panel_presences: HashMap<String, Vec<String>>,
    user_presences: HashMap<String, UserPresence>,
    on_state_change: Option<Box<dyn Fn(&str, &serde_json::Value) + Send + Sync>>,
}

impl RegistryInner {
    fn new() -> Self {
        Self {
            elements: HashMap::new(),
            panel_presences: HashMap::new(),
            user_presences: HashMap::new(),
            on_state_change: None,
        }
    }
}

/// Global registry of all replicated elements in the application
pub struct ReplicationRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

impl Global for ReplicationRegistry {}

/// State tracking for a single replicated element
#[derive(Debug, Clone)]
pub struct ElementReplicationState {
    pub element_id: String,
    pub config: ReplicationConfig,
    pub last_state: Option<serde_json::Value>,
    pub last_update: u64,
    pub active_editors: Vec<String>,
    pub locked_by: Option<String>,
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

    pub fn can_edit(&self, peer_id: &str) -> bool {
        match self.config.mode {
            ReplicationMode::NoRep => true,
            ReplicationMode::MultiEdit => {
                if let Some(max) = self.config.max_concurrent_editors {
                    if self.active_editors.len() >= max
                        && !self.active_editors.contains(&peer_id.to_string())
                    {
                        return false;
                    }
                }
                true
            }
            ReplicationMode::LockedEdit => {
                self.locked_by.is_none() || self.locked_by.as_deref() == Some(peer_id)
            }
            ReplicationMode::RequestEdit => self.active_editors.contains(&peer_id.to_string()),
            ReplicationMode::BroadcastOnly => peer_id == "0",
            ReplicationMode::Follow => true,
            ReplicationMode::QueuedEdit => true,
            ReplicationMode::PartitionedEdit => true,
        }
    }

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

    pub fn release_lock(&mut self, peer_id: &str) {
        if self.locked_by.as_deref() == Some(peer_id) {
            self.locked_by = None;
        }
    }

    pub fn request_permission(&mut self, peer_id: &str) {
        if self.config.mode != ReplicationMode::RequestEdit {
            return;
        }
        if !self.pending_requests.contains(&peer_id.to_string()) {
            self.pending_requests.push(peer_id.to_string());
        }
    }

    pub fn grant_permission(&mut self, peer_id: &str) {
        self.pending_requests.retain(|id| id != peer_id);
        if !self.active_editors.contains(&peer_id.to_string()) {
            self.active_editors.push(peer_id.to_string());
        }
    }

    pub fn revoke_permission(&mut self, peer_id: &str) {
        self.active_editors.retain(|id| id != peer_id);
    }
}

impl ReplicationRegistry {
    pub fn init(cx: &mut App) {
        cx.set_global(Self {
            inner: Arc::new(RwLock::new(RegistryInner::new())),
        });
    }

    pub fn global(cx: &App) -> Self {
        cx.global::<Self>().clone()
    }

    pub fn register_element(&self, element_id: String, config: ReplicationConfig) {
        let state = ElementReplicationState::new(element_id.clone(), config);
        self.inner.write().elements.insert(element_id, state);
    }

    pub fn unregister_element(&self, element_id: &str) {
        self.inner.write().elements.remove(element_id);
    }

    pub fn get_element_state(&self, element_id: &str) -> Option<ElementReplicationState> {
        self.inner.read().elements.get(element_id).cloned()
    }

    pub fn update_element_state(
        &self,
        element_id: &str,
        state: serde_json::Value,
        timestamp: u64,
    ) -> bool {
        let mut inner = self.inner.write();
        if let Some(elem_state) = inner.elements.get_mut(element_id) {
            elem_state.last_state = Some(state.clone());
            elem_state.last_update = timestamp;
            if let Some(callback) = inner.on_state_change.as_ref() {
                callback(element_id, &state);
            }
            true
        } else {
            false
        }
    }

    pub fn add_editor(&self, element_id: &str, peer_id: &str) -> bool {
        let mut inner = self.inner.write();
        if let Some(state) = inner.elements.get_mut(element_id) {
            if state.can_edit(peer_id) && !state.active_editors.contains(&peer_id.to_string()) {
                state.active_editors.push(peer_id.to_string());
                return true;
            }
        }
        false
    }

    pub fn remove_editor(&self, element_id: &str, peer_id: &str) {
        let mut inner = self.inner.write();
        if let Some(state) = inner.elements.get_mut(element_id) {
            state.active_editors.retain(|id| id != peer_id);
            state.release_lock(peer_id);
        }
    }

    pub fn get_editors(&self, element_id: &str) -> Vec<String> {
        self.inner
            .read()
            .elements
            .get(element_id)
            .map(|state| state.active_editors.clone())
            .unwrap_or_default()
    }

    pub fn add_panel_presence(&self, panel_id: &str, peer_id: &str) {
        self.inner
            .write()
            .panel_presences
            .entry(panel_id.to_string())
            .or_insert_with(Vec::new)
            .push(peer_id.to_string());
    }

    pub fn remove_panel_presence(&self, panel_id: &str, peer_id: &str) {
        let mut inner = self.inner.write();
        if let Some(users) = inner.panel_presences.get_mut(panel_id) {
            users.retain(|id| id != peer_id);
            if users.is_empty() {
                inner.panel_presences.remove(panel_id);
            }
        }
    }

    pub fn get_panel_users(&self, panel_id: &str) -> Vec<String> {
        self.inner
            .read()
            .panel_presences
            .get(panel_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn update_user_presence(&self, presence: UserPresence) {
        self.inner
            .write()
            .user_presences
            .insert(presence.peer_id.clone(), presence);
    }

    pub fn get_user_presence(&self, peer_id: &str) -> Option<UserPresence> {
        self.inner.read().user_presences.get(peer_id).cloned()
    }

    pub fn get_all_presences(&self) -> Vec<UserPresence> {
        self.inner.read().user_presences.values().cloned().collect()
    }

    pub fn remove_user_presence(&self, peer_id: &str) {
        let mut inner = self.inner.write();
        inner.user_presences.remove(peer_id);
        for users in inner.panel_presences.values_mut() {
            users.retain(|id| id != peer_id);
        }
        inner.panel_presences.retain(|_, users| !users.is_empty());
        for state in inner.elements.values_mut() {
            state.active_editors.retain(|id| id != peer_id);
            if state.locked_by.as_deref() == Some(peer_id) {
                state.locked_by = None;
            }
            state.pending_requests.retain(|id| id != peer_id);
        }
    }

    pub fn on_state_change<F>(&self, callback: F)
    where
        F: Fn(&str, &serde_json::Value) + Send + Sync + 'static,
    {
        self.inner.write().on_state_change = Some(Box::new(callback));
    }

    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.elements.clear();
        inner.panel_presences.clear();
        inner.user_presences.clear();
    }
}

impl Clone for ReplicationRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
