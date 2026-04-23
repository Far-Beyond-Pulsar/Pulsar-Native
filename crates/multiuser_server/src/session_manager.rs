//! Session lifecycle management and JWT token handling.

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::rendezvous::peer_discovery::{PeerSession, RendezvousSession};


/// Rendezvous coordinator state
#[derive(Clone)]
pub struct RendezvousCoordinator {
    config: Config,
    sessions: Arc<DashMap<String, Arc<RendezvousSession>>>,
    jwt_encoding_key: EncodingKey,
    jwt_decoding_key: DecodingKey,
}

/// JWT Claims structure for session tokens
#[derive(Debug, Serialize, Deserialize)]
struct TokenClaims {
    sub: String,      // Subject (peer_id)
    session: String,  // Session ID
    exp: usize,       // Expiration time
    iat: usize,       // Issued at
}

impl RendezvousCoordinator {
    /// Create a new rendezvous coordinator
    pub fn new(config: Config) -> Self {
        let jwt_secret = config.jwt_secret.as_bytes();
        Self {
            jwt_encoding_key: EncodingKey::from_secret(jwt_secret),
            jwt_decoding_key: DecodingKey::from_secret(jwt_secret),
            config,
            sessions: Arc::new(DashMap::new()),
        }
    }

    /// Validate JWT token
    fn validate_jwt_token(&self, token: &str) -> Result<TokenClaims> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;

        let token_data = decode::<TokenClaims>(
            token,
            &self.jwt_decoding_key,
            &validation,
        )
        .context("Failed to decode JWT token")?;

        Ok(token_data.claims)
    }

    /// Generate JWT token for a peer joining a session
    pub fn generate_join_token(&self, peer_id: &str, session_id: &str, ttl_secs: u64) -> Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as usize;

        let claims = TokenClaims {
            sub: peer_id.to_string(),
            session: session_id.to_string(),
            iat: now,
            exp: now + ttl_secs as usize,
        };

        encode(&Header::default(), &claims, &self.jwt_encoding_key)
            .context("Failed to encode JWT token")
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


