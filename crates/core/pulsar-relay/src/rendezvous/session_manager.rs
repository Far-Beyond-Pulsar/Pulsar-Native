//! Session lifecycle management and JWT token handling.

use anyhow::{Context, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

use super::peer_discovery::RendezvousSession;
use super::sync_protocol::ServerMessage;
use crate::auth::AuthService;
use crate::config::Config;
use crate::metrics::METRICS;

/// Rendezvous coordinator state
#[derive(Clone)]
pub struct RendezvousCoordinator {
    pub(super) auth: Arc<AuthService>,
    pub(super) config: Config,
    pub(super) sessions: Arc<DashMap<String, Arc<RendezvousSession>>>,
}

impl RendezvousCoordinator {
    /// Create a new rendezvous coordinator
    pub fn new(auth: Arc<AuthService>, config: Config) -> Self {
        Self {
            auth,
            config,
            sessions: Arc::new(DashMap::new()),
        }
    }

    /// Create a new rendezvous session
    pub fn create_session(&self, session_id: String, host_id: String) -> Result<()> {
        let session = Arc::new(RendezvousSession::new(session_id.clone(), host_id));
        self.sessions.insert(session_id.clone(), session);

        info!(session = %session_id, "Created rendezvous session");
        METRICS.sessions_active.inc();

        Ok(())
    }

    /// Close a rendezvous session
    pub fn close_session(&self, session_id: &str) -> Result<()> {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            let peer_count = session.peer_count();
            info!(
                session = %session_id,
                peers = peer_count,
                "Closed rendezvous session"
            );

            // Notify all peers that session is closing
            for peer in session.list_peers() {
                let _ = peer.tx.try_send(ServerMessage::Error {
                    message: "Session closed".to_string(),
                });
            }

            METRICS.sessions_active.dec();
        }

        Ok(())
    }
}
