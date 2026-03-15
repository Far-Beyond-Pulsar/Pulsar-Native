use std::{collections::HashMap, sync::Arc};
use parking_lot::RwLock;
use tokio::sync::broadcast;
use tracing::info;

use super::types::{SessionHandle, WsMessage};

/// Capacity of the broadcast channel per active session.
const CHANNEL_CAPACITY: usize = 256;

/// Thread-safe registry of running project sessions and connected users.
#[derive(Debug, Clone)]
pub struct SessionManager {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Map from project_id → active session channel.
    sessions: HashMap<String, SessionHandle>,
    /// Map from project_id → list of connected usernames.
    users: HashMap<String, Vec<String>>,
}

#[allow(dead_code)]
impl SessionManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner::default())),
        }
    }

    // ── Session lifecycle ─────────────────────────────────────────────────────

    /// Ensure a broadcast channel exists for `project_id`. Returns the channel
    /// sender regardless of whether it was just created or already existed.
    pub fn get_or_create_session(&self, project_id: &str) -> broadcast::Sender<WsMessage> {
        let mut guard = self.inner.write();
        if let Some(handle) = guard.sessions.get(project_id) {
            return handle.tx.clone();
        }
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        guard.sessions.insert(project_id.to_string(), SessionHandle {
            project_id: project_id.to_string(),
            tx: tx.clone(),
        });
        tx
    }

    /// Remove the session for a project if it has no remaining subscribers.
    pub fn cleanup_if_empty(&self, project_id: &str) -> bool {
        let mut guard = self.inner.write();
        if guard.users.get(project_id).map_or(true, |u| u.is_empty()) {
            guard.sessions.remove(project_id);
            guard.users.remove(project_id);
            return true;
        }
        false
    }

    // ── User tracking ─────────────────────────────────────────────────────────

    /// Register a user joining a project. Returns the broadcast sender and the
    /// current user list to send as a `UserList` message.
    pub fn user_joined(&self, project_id: &str, username: &str) -> (broadcast::Sender<WsMessage>, Vec<String>) {
        let tx = self.get_or_create_session(project_id);
        let mut guard = self.inner.write();
        let users = guard.users.entry(project_id.to_string()).or_default();
        if !users.contains(&username.to_string()) {
            users.push(username.to_string());
        }
        let user_list = users.clone();

        // Broadcast the join event to existing members.
        let _ = tx.send(WsMessage::UserJoined { user: username.to_string() });
        info!("User '{}' joined project '{}'", username, project_id);

        (tx, user_list)
    }

    /// Remove a user from a project session. Broadcasts `UserLeft`.
    pub fn user_left(&self, project_id: &str, username: &str) {
        let mut guard = self.inner.write();
        if let Some(users) = guard.users.get_mut(project_id) {
            users.retain(|u| u != username);
        }

        // Broadcast the leave event.
        if let Some(handle) = guard.sessions.get(project_id) {
            let _ = handle.tx.send(WsMessage::UserLeft { user: username.to_string() });
        }

        info!("User '{}' left project '{}'", username, project_id);
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    pub fn user_count(&self, project_id: &str) -> usize {
        self.inner.read().users.get(project_id).map_or(0, |u| u.len())
    }

    pub fn total_user_count(&self) -> usize {
        self.inner.read().users.values().map(|u| u.len()).sum()
    }

    pub fn active_project_count(&self) -> usize {
        self.inner.read().sessions.len()
    }

    pub fn users_in_project(&self, project_id: &str) -> Vec<String> {
        self.inner.read().users.get(project_id).cloned().unwrap_or_default()
    }

    /// Broadcast a [`WsMessage::FileChanged`] event to all subscribers of
    /// `project_id`.  Silently does nothing when there is no active session
    /// (i.e. no connected users).
    pub fn broadcast_file_change(&self, project_id: &str, path: String, kind: String) {
        let guard = self.inner.read();
        if let Some(handle) = guard.sessions.get(project_id) {
            let _ = handle.tx.send(WsMessage::FileChanged { path, kind });
        }
    }
}
