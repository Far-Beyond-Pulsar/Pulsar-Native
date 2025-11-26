//! WebSocket signaling coordinator
//!
//! This module handles WebSocket-based signaling for session coordination,
//! PunchCoord orchestration, candidate exchange, and peer routing.

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
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::metrics::METRICS;
use crate::nat::{ConnectionCandidate, NatType};

/// Messages sent FROM client TO server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Join a session
    Join {
        session_id: String,
        peer_id: String,
        join_token: String,
    },
    /// Leave a session
    Leave {
        session_id: String,
        peer_id: String,
    },
    /// Chat message
    ChatMessage {
        session_id: String,
        peer_id: String,
        message: String,
    },
    /// Request file manifest from host
    RequestFileManifest {
        session_id: String,
        peer_id: String,
    },
    /// Send file manifest (host response)
    FileManifest {
        session_id: String,
        peer_id: String,
        manifest_json: String,
    },
    /// Request specific files
    RequestFiles {
        session_id: String,
        peer_id: String,
        file_paths: Vec<String>,
    },
    /// Send file data chunk
    FilesChunk {
        session_id: String,
        peer_id: String,
        files_json: String,
        chunk_index: usize,
        total_chunks: usize,
    },
    /// Heartbeat / keepalive
    Ping,
}

/// Messages sent FROM server TO client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Confirmation that client joined
    Joined {
        session_id: String,
        peer_id: String,
        participants: Vec<String>,
    },
    /// Another peer joined
    PeerJoined {
        session_id: String,
        peer_id: String,
    },
    /// A peer left
    PeerLeft {
        session_id: String,
        peer_id: String,
    },
    /// Chat message (relayed)
    ChatMessage {
        session_id: String,
        peer_id: String,
        message: String,
        timestamp: u64,
    },
    /// File manifest request (relayed from guest to host)
    RequestFileManifest {
        session_id: String,
        from_peer_id: String,
    },
    /// File manifest response (relayed from host to guest)
    FileManifest {
        session_id: String,
        from_peer_id: String,
        manifest_json: String,
    },
    /// File request (relayed from guest to host)
    RequestFiles {
        session_id: String,
        from_peer_id: String,
        file_paths: Vec<String>,
    },
    /// Files chunk (relayed from host to guest)
    FilesChunk {
        session_id: String,
        from_peer_id: String,
        files_json: String,
        chunk_index: usize,
        total_chunks: usize,
    },
    /// Heartbeat response
    Pong,
    /// Error message
    Error { message: String },
}

/// Candidate DTO for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateDto {
    pub ip: String,
    pub port: u16,
    pub proto: String,
    pub priority: u32,
    pub candidate_type: String,
}

impl From<ConnectionCandidate> for CandidateDto {
    fn from(c: ConnectionCandidate) -> Self {
        Self {
            ip: c.addr.ip().to_string(),
            port: c.addr.port(),
            proto: c.proto,
            priority: c.priority,
            candidate_type: c.candidate_type.to_string(),
        }
    }
}

/// Peer session information
#[derive(Debug, Clone)]
struct PeerSession {
    peer_id: String,
    session_id: String,
    tx: mpsc::Sender<ServerMessage>,
    joined_at: SystemTime,
}

/// Rendezvous session
#[derive(Debug)]
struct RendezvousSession {
    session_id: String,
    host_id: String,
    peers: DashMap<String, PeerSession>,
    created_at: SystemTime,
}

impl RendezvousSession {
    fn new(session_id: String, host_id: String) -> Self {
        Self {
            session_id,
            host_id,
            peers: DashMap::new(),
            created_at: SystemTime::now(),
        }
    }

    fn add_peer(&self, peer: PeerSession) {
        self.peers.insert(peer.peer_id.clone(), peer);
    }

    fn remove_peer(&self, peer_id: &str) -> Option<PeerSession> {
        self.peers.remove(peer_id).map(|(_, p)| p)
    }

    fn get_peer(&self, peer_id: &str) -> Option<PeerSession> {
        self.peers.get(peer_id).map(|p| p.clone())
    }

    fn list_peers(&self) -> Vec<PeerSession> {
        self.peers.iter().map(|p| p.value().clone()).collect()
    }

    fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

/// Rendezvous coordinator state
#[derive(Clone)]
pub struct RendezvousCoordinator {
    config: Config,
    sessions: Arc<DashMap<String, Arc<RendezvousSession>>>,
}

impl RendezvousCoordinator {
    /// Create a new rendezvous coordinator
    pub fn new(config: Config) -> Self {
        Self {
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

    /// Handle WebSocket upgrade
    pub async fn handle_websocket(
        State(coordinator): State<Arc<Self>>,
        ws: WebSocketUpgrade,
    ) -> Response {
        ws.on_upgrade(move |socket| coordinator.handle_socket(socket))
    }

    async fn handle_socket(self: Arc<Self>, socket: WebSocket) {
        let (mut sender, mut receiver) = socket.split();

        // Create channel for outgoing messages
        let (tx, mut rx) = mpsc::channel::<ServerMessage>(100);

        // Spawn task to send messages to WebSocket
        let send_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(e) => {
                        error!(error = %e, "Failed to serialize message");
                        continue;
                    }
                };

                if sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        });

        // Track peer state
        let mut peer_id: Option<String> = None;
        let mut session_id: Option<String> = None;

        // Receive messages from WebSocket
        while let Some(msg) = receiver.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, "WebSocket error");
                    break;
                }
            };

            match msg {
                Message::Text(text) => {
                    METRICS
                        .signaling_messages
                        .with_label_values(&["received"])
                        .inc();

                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(client_msg) => {
                            let result = self.handle_client_message(
                                client_msg,
                                tx.clone(),
                                &mut peer_id,
                                &mut session_id,
                            ).await;

                            if let Err(e) = result {
                                error!(error = %e, "Failed to handle client message");
                                let _ = tx.send(ServerMessage::Error {
                                    message: e.to_string(),
                                }).await;
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, text = %text, "Failed to parse client message");
                        }
                    }
                }
                Message::Ping(_) => {
                    // Automatically handled by axum
                }
                Message::Close(_) => {
                    debug!("WebSocket closed by peer");
                    break;
                }
                _ => {}
            }
        }

        // Cleanup on disconnect
        if let (Some(pid), Some(sid)) = (peer_id, session_id) {
            self.handle_peer_disconnect(&sid, &pid).await;
        }

        send_task.abort();
    }

    async fn handle_client_message(
        &self,
        msg: ClientMessage,
        tx: mpsc::Sender<ServerMessage>,
        peer_id: &mut Option<String>,
        session_id: &mut Option<String>,
    ) -> Result<()> {
        match msg {
            ClientMessage::Join {
                session_id: sid,
                peer_id: pid,
                join_token,
            } => {
                self.handle_join(sid, pid, join_token, tx, peer_id, session_id)
                    .await?;
            }
            ClientMessage::Leave {
                session_id: sid,
                peer_id: pid,
            } => {
                self.handle_leave(&sid, &pid).await?;
            }
            ClientMessage::ChatMessage {
                session_id: sid,
                peer_id: pid,
                message,
            } => {
                self.relay_chat_message(&sid, &pid, message).await?;
            }
            ClientMessage::RequestFileManifest {
                session_id: sid,
                peer_id: pid,
            } => {
                self.forward_to_host(&sid, ServerMessage::RequestFileManifest {
                    session_id: sid,
                    from_peer_id: pid,
                }).await?;
            }
            ClientMessage::FileManifest {
                session_id: sid,
                peer_id: pid,
                manifest_json,
            } => {
                self.relay_file_manifest(&sid, &pid, manifest_json).await?;
            }
            ClientMessage::RequestFiles {
                session_id: sid,
                peer_id: pid,
                file_paths,
            } => {
                self.forward_to_host(&sid, ServerMessage::RequestFiles {
                    session_id: sid,
                    from_peer_id: pid,
                    file_paths,
                }).await?;
            }
            ClientMessage::FilesChunk {
                session_id: sid,
                peer_id: pid,
                files_json,
                chunk_index,
                total_chunks,
            } => {
                self.relay_files_chunk(&sid, &pid, files_json, chunk_index, total_chunks).await?;
            }
            ClientMessage::Ping => {
                tx.send(ServerMessage::Pong).await?;
            }
        }

        Ok(())
    }

    async fn handle_join(
        &self,
        sid: String,
        pid: String,
        _join_token: String,
        candidates: Vec<CandidateDto>,
        pubkey: Vec<u8>,
        tx: mpsc::Sender<SignalingMessage>,
        peer_id: &mut Option<String>,
        session_id: &mut Option<String>,
    ) -> Result<()> {
        // Validate join token (simplified - should use JWT verification)
        // TODO: Implement proper token validation

        let session = self
            .sessions
            .get(&sid)
            .context("Session not found")?
            .clone();

        let peer = PeerSession {
            peer_id: pid.clone(),
            session_id: sid.clone(),
            tx,
            candidates: candidates.clone(),
            pubkey,
            joined_at: SystemTime::now(),
            nat_type: None,
        };

        session.add_peer(peer);
        *peer_id = Some(pid.clone());
        *session_id = Some(sid.clone());

        info!(session = %sid, peer = %pid, "Peer joined session");

        // Send punch coordination to all peers
        self.coordinate_punch(&sid, &pid).await?;

        // Notify other peers
        self.broadcast_peer_joined(&sid, &pid, &candidates).await?;

        Ok(())
    }

    async fn handle_leave(&self, sid: &str, pid: &str) -> Result<()> {
        if let Some(session) = self.sessions.get(sid) {
            session.remove_peer(pid);
            info!(session = %sid, peer = %pid, "Peer left session");

            // Notify other peers
            self.broadcast_peer_left(sid, pid).await?;
        }

        Ok(())
    }

    async fn handle_candidate(&self, sid: &str, pid: &str, candidate: CandidateDto) -> Result<()> {
        debug!(
            session = %sid,
            peer = %pid,
            candidate = ?candidate,
            "Received candidate"
        );

        // Broadcast candidate to other peers in session
        self.broadcast_candidate(sid, pid, candidate).await?;

        Ok(())
    }

    async fn coordinate_punch(&self, sid: &str, pid: &str) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let peer = session.get_peer(pid).context("Peer not found")?;

        // Generate punch coordination token
        let token = self.generate_punch_token(sid, pid)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let expires = now + self.config.hole_punch_timeout.as_secs() as i64;

        let punch_msg = SignalingMessage::PunchCoord {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
            token,
            start_ts: now,
            expires,
            candidates: peer.candidates.clone(),
        };

        // Send to all other peers
        for other_peer in session.list_peers() {
            if other_peer.peer_id != pid {
                let _ = other_peer.tx.send(punch_msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["punch_coord"])
            .inc();

        Ok(())
    }

    fn generate_punch_token(&self, session_id: &str, peer_id: &str) -> Result<Vec<u8>> {
        // Generate HMAC token for hole punching
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(self.config.jwt_secret.as_bytes())
            .context("Failed to create HMAC")?;

        mac.update(session_id.as_bytes());
        mac.update(peer_id.as_bytes());
        mac.update(&SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_le_bytes());

        Ok(mac.finalize().into_bytes().to_vec())
    }

    async fn forward_to_peer(&self, sid: &str, pid: &str, msg: SignalingMessage) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;
        let peer = session.get_peer(pid).context("Peer not found")?;

        peer.tx
            .send(msg)
            .await
            .context("Failed to send message to peer")?;

        METRICS
            .signaling_messages
            .with_label_values(&["forwarded"])
            .inc();

        Ok(())
    }

    async fn broadcast_peer_joined(
        &self,
        sid: &str,
        pid: &str,
        candidates: &[CandidateDto],
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = SignalingMessage::Candidate {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
            candidate: candidates.first().cloned().unwrap_or(CandidateDto {
                ip: "0.0.0.0".to_string(),
                port: 0,
                proto: "udp".to_string(),
                priority: 0,
                candidate_type: "host".to_string(),
            }),
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        Ok(())
    }

    async fn broadcast_peer_left(&self, sid: &str, pid: &str) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = SignalingMessage::Leave {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        Ok(())
    }

    async fn broadcast_candidate(
        &self,
        sid: &str,
        pid: &str,
        candidate: CandidateDto,
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = SignalingMessage::Candidate {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
            candidate,
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        Ok(())
    }

    async fn handle_peer_disconnect(&self, sid: &str, pid: &str) {
        info!(session = %sid, peer = %pid, "Peer disconnected");

        if let Err(e) = self.handle_leave(sid, pid).await {
            error!(error = %e, "Failed to handle peer disconnect");
        }
    }

    /// Background task to clean up stale sessions
    pub async fn cleanup_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            let session_ttl = self.config.session_ttl;
            let now = SystemTime::now();
            let mut to_remove = Vec::new();

            for entry in self.sessions.iter() {
                let session = entry.value();
                if let Ok(elapsed) = now.duration_since(session.created_at) {
                    if elapsed > session_ttl && session.peer_count() == 0 {
                        to_remove.push(entry.key().clone());
                    }
                }
            }

            for session_id in to_remove {
                if let Err(e) = self.close_session(&session_id) {
                    error!(error = %e, session = %session_id, "Failed to close stale session");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rendezvous_session_creation() {
        let session = RendezvousSession::new("test-session".to_string(), "host-1".to_string());
        assert_eq!(session.session_id, "test-session");
        assert_eq!(session.host_id, "host-1");
        assert_eq!(session.peer_count(), 0);
    }

    #[test]
    fn test_coordinator_creation() {
        let config = Config::default();
        let coordinator = RendezvousCoordinator::new(config);
        assert_eq!(coordinator.sessions.len(), 0);
    }

    #[test]
    fn test_session_lifecycle() {
        let config = Config::default();
        let coordinator = RendezvousCoordinator::new(config);

        let session_id = Uuid::new_v4().to_string();
        let host_id = Uuid::new_v4().to_string();

        coordinator
            .create_session(session_id.clone(), host_id)
            .unwrap();
        assert_eq!(coordinator.sessions.len(), 1);

        coordinator.close_session(&session_id).unwrap();
        assert_eq!(coordinator.sessions.len(), 0);
    }
}
