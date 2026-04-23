//! Session management and lifecycle
//!
//! Handles creation, joining, leaving, and garbage collection of collaborative editing sessions.
//! Each session has a unique ID, host, participants, and metadata for coordination.

use anyhow::{Context, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::auth::Role;
use crate::config::Config;
use crate::metrics::METRICS;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub host_id: String,
    pub participants: Vec<ParticipantInfo>,
    pub created_at: u64,
    pub expires_at: u64,
    pub session_key: Vec<u8>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub peer_id: String,
    pub role: Role,
    pub joined_at: u64,
    pub last_seen: u64,
}

pub struct SessionStore {
    sessions: Arc<DashMap<String, Session>>,
    config: Arc<Config>,
}

impl SessionStore {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            config,
        }
    }

    /// Create a new session with a specific ID (for client-generated sessions)
    pub fn create_session_with_id(
        &self,
        session_id: String,
        host_id: String,
        metadata: serde_json::Value,
    ) -> Result<Session> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        // Generate session key (random 32 bytes)
        let session_key: Vec<u8> = (0..32).map(|_| rand::random()).collect();

        let session = Session {
            id: session_id.clone(),
            host_id: host_id.clone(),
            participants: vec![ParticipantInfo {
                peer_id: host_id,
                role: Role::Host,
                joined_at: now,
                last_seen: now,
            }],
            created_at: now,
            expires_at: now + self.config.session_ttl.as_secs(),
            session_key,
            metadata,
        };

        self.sessions.insert(session_id.clone(), session.clone());

        METRICS
            .sessions_total
            .with_label_values(&[&session.host_id])
            .inc();
        METRICS.sessions_active.inc();

        info!(
            session_id = %session_id,
            host_id = %session.host_id,
            participants = session.participants.len(),
            "✨ Session created"
        );

        Ok(session)
    }

    /// Create a new session (generates a random UUID for session ID)
    pub fn create_session(&self, host_id: String, metadata: serde_json::Value) -> Result<Session> {
        let session_id = Uuid::new_v4().to_string();
        self.create_session_with_id(session_id, host_id, metadata)
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &str) -> Option<Session> {
        self.sessions.get(session_id).map(|s| s.clone())
    }

    /// Add a participant to a session
    pub fn join_session(&self, session_id: &str, peer_id: String, role: Role) -> Result<Session> {
        let mut session = self
            .sessions
            .get_mut(session_id)
            .context("Session not found")?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        // Check if peer already in session
        if session.participants.iter().any(|p| p.peer_id == peer_id) {
            anyhow::bail!("Peer already in session");
        }

        session.participants.push(ParticipantInfo {
            peer_id: peer_id.clone(),
            role: role.clone(),
            joined_at: now,
            last_seen: now,
        });

        info!(
            session_id = %session_id,
            peer_id = %peer_id,
            role = ?role,
            total_participants = session.participants.len(),
            "👤 Peer joined session"
        );

        Ok(session.clone())
    }

    /// Remove a participant from a session
    pub fn leave_session(&self, session_id: &str, peer_id: &str) -> Result<()> {
        let mut session = self
            .sessions
            .get_mut(session_id)
            .context("Session not found")?;

        session.participants.retain(|p| p.peer_id != peer_id);

        info!(
            session_id = %session_id,
            peer_id = %peer_id,
            remaining_participants = session.participants.len(),
            "👋 Peer left session"
        );

        // Close session if host left or no participants remain
        if session.participants.is_empty()
            || !session
                .participants
                .iter()
                .any(|p| p.peer_id == session.host_id)
        {
            drop(session);
            self.close_session(session_id, "host_left")?;
        }

        Ok(())
    }

    /// Update participant's last_seen timestamp
    pub fn update_last_seen(&self, session_id: &str, peer_id: &str) -> Result<()> {
        let mut session = self
            .sessions
            .get_mut(session_id)
            .context("Session not found")?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        if let Some(participant) = session
            .participants
            .iter_mut()
            .find(|p| p.peer_id == peer_id)
        {
            participant.last_seen = now;
        }

        Ok(())
    }

    /// Close a session
    pub fn close_session(&self, session_id: &str, reason: &str) -> Result<()> {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            METRICS.sessions_active.dec();
            METRICS.sessions_closed.with_label_values(&[reason]).inc();

            let duration = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                - session.created_at;

            info!(
                session_id = %session_id,
                reason = %reason,
                duration_secs = duration,
                participants = session.participants.len(),
                "🔒 Session closed"
            );
        }

        Ok(())
    }

    /// List all active sessions
    pub fn list_sessions(&self) -> Vec<Session> {
        self.sessions
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get session count
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Garbage collect expired sessions
    pub async fn garbage_collect(&self) -> Result<usize> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let mut removed = 0;

        let expired_sessions: Vec<String> = self
            .sessions
            .iter()
            .filter(|entry| entry.value().expires_at < now)
            .map(|entry| entry.key().clone())
            .collect();

        for session_id in expired_sessions {
            self.close_session(&session_id, "expired")?;
            removed += 1;
        }

        if removed > 0 {
            info!(removed = removed, "🧹 Garbage collected expired sessions");
        }

        Ok(removed)
    }

    /// Remove stale participants (haven't sent heartbeat in a while)
    pub async fn remove_stale_participants(&self, timeout: Duration) -> Result<usize> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let timeout_secs = timeout.as_secs();
        let mut removed = 0;

        for mut session in self.sessions.iter_mut() {
            let stale_peers: Vec<String> = session
                .participants
                .iter()
                .filter(|p| now - p.last_seen > timeout_secs)
                .map(|p| p.peer_id.clone())
                .collect();

            for peer_id in stale_peers {
                session.participants.retain(|p| p.peer_id != peer_id);
                removed += 1;

                warn!(
                    session_id = %session.id,
                    peer_id = %peer_id,
                    "⚠️  Removed stale participant (timeout)"
                );
            }
        }

        if removed > 0 {
            info!(
                removed = removed,
                "🧹 Removed {} stale participants", removed
            );
        }

        Ok(removed)
    }
}

/// Background garbage collector loop
pub async fn garbage_collector_loop(store: Arc<SessionStore>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);

    loop {
        ticker.tick().await;

        if let Err(e) = store.garbage_collect().await {
            tracing::error!(error = %e, "Error during garbage collection");
        }

        if let Err(e) = store
            .remove_stale_participants(Duration::from_secs(300))
            .await
        {
            tracing::error!(error = %e, "Error removing stale participants");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Arc<Config> {
        Arc::new(Config::default())
    }

    #[test]
    fn test_create_session() {
        let store = SessionStore::new(test_config());
        let session = store
            .create_session("host123".to_string(), serde_json::json!({}))
            .unwrap();

        assert_eq!(session.host_id, "host123");
        assert_eq!(session.participants.len(), 1);
        assert_eq!(session.participants[0].peer_id, "host123");
    }

    #[test]
    fn test_join_session() {
        let store = SessionStore::new(test_config());
        let session = store
            .create_session("host123".to_string(), serde_json::json!({}))
            .unwrap();

        let updated = store
            .join_session(&session.id, "peer456".to_string(), Role::Editor)
            .unwrap();

        assert_eq!(updated.participants.len(), 2);
    }

    #[test]
    fn test_leave_session() {
        let store = SessionStore::new(test_config());
        let session = store
            .create_session("host123".to_string(), serde_json::json!({}))
            .unwrap();

        store
            .join_session(&session.id, "peer456".to_string(), Role::Editor)
            .unwrap();

        store.leave_session(&session.id, "peer456").unwrap();

        let session = store.get_session(&session.id).unwrap();
        assert_eq!(session.participants.len(), 1);
    }

    #[tokio::test]
    async fn test_garbage_collect() {
        let mut config = Config::default();
        config.session_ttl = Duration::from_secs(1);
        let store = SessionStore::new(Arc::new(config));

        store
            .create_session("host123".to_string(), serde_json::json!({}))
            .unwrap();

        assert_eq!(store.session_count(), 1);

        // Wait for expiration
        tokio::time::sleep(Duration::from_secs(2)).await;

        let removed = store.garbage_collect().await.unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.session_count(), 0);
    }
}
