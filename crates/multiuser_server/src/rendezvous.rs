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
    /// Kick a user (host only)
    KickUser {
        session_id: String,
        peer_id: String,
        target_peer_id: String,
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
    /// P2P connection negotiation
    P2PConnectionRequest {
        session_id: String,
        peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    P2PConnectionResponse {
        session_id: String,
        peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    /// Binary proxy mode (raw bytes, no JSON)
    RequestBinaryProxy {
        session_id: String,
        peer_id: String,
    },
    BinaryProxyData {
        session_id: String,
        peer_id: String,
        sequence: u64,
        is_git_protocol: bool,
    },
    /// Git sync messages
    RequestProjectTree {
        session_id: String,
        peer_id: String,
    },
    ProjectTreeResponse {
        session_id: String,
        peer_id: String,
        tree_json: String,
    },
    /// Legacy file-based messages
    RequestFile {
        session_id: String,
        peer_id: String,
        file_path: String,
    },
    FileChunk {
        session_id: String,
        peer_id: String,
        file_path: String,
        offset: u64,
        data: Vec<u8>,
        is_last: bool,
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
    /// You were kicked from the session
    Kicked {
        session_id: String,
        reason: String,
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
    /// P2P connection negotiation (relayed)
    P2PConnectionRequest {
        session_id: String,
        from_peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    P2PConnectionResponse {
        session_id: String,
        from_peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    /// Binary proxy (relayed)
    RequestBinaryProxy {
        session_id: String,
        from_peer_id: String,
    },
    BinaryProxyData {
        session_id: String,
        from_peer_id: String,
        sequence: u64,
        is_git_protocol: bool,
    },
    /// Git sync messages (relayed)
    RequestProjectTree {
        session_id: String,
        from_peer_id: String,
    },
    ProjectTreeResponse {
        session_id: String,
        from_peer_id: String,
        tree_json: String,
    },
    /// Legacy file messages
    RequestFile {
        session_id: String,
        from_peer_id: String,
        file_path: String,
    },
    FileChunk {
        session_id: String,
        from_peer_id: String,
        file_path: String,
        offset: u64,
        data: Vec<u8>,
        is_last: bool,
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

impl PeerSession {
    fn new(peer_id: String, session_id: String, tx: mpsc::Sender<ServerMessage>) -> Self {
        Self {
            peer_id,
            session_id,
            tx,
            joined_at: SystemTime::now(),
        }
    }
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
            ClientMessage::KickUser {
                session_id: sid,
                peer_id: pid,
                target_peer_id,
            } => {
                self.handle_kick(&sid, &pid, target_peer_id).await?;
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
                    session_id: sid.clone(),
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
                    session_id: sid.clone(),
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
            ClientMessage::P2PConnectionRequest {
                session_id: sid,
                peer_id: pid,
                public_ip,
                public_port,
            } => {
                self.relay_p2p_request(&sid, &pid, public_ip, public_port).await?;
            }
            ClientMessage::P2PConnectionResponse {
                session_id: sid,
                peer_id: pid,
                public_ip,
                public_port,
            } => {
                self.relay_p2p_response(&sid, &pid, public_ip, public_port).await?;
            }
            ClientMessage::RequestBinaryProxy {
                session_id: sid,
                peer_id: pid,
            } => {
                self.relay_binary_proxy_request(&sid, &pid).await?;
            }
            ClientMessage::BinaryProxyData {
                session_id: sid,
                peer_id: pid,
                sequence,
                is_git_protocol,
            } => {
                self.relay_binary_proxy_data(&sid, &pid, sequence, is_git_protocol).await?;
            }
            ClientMessage::RequestProjectTree {
                session_id: sid,
                peer_id: pid,
            } => {
                self.forward_to_host(&sid, ServerMessage::RequestProjectTree {
                    session_id: sid.clone(),
                    from_peer_id: pid,
                }).await?;
            }
            ClientMessage::ProjectTreeResponse {
                session_id: sid,
                peer_id: pid,
                tree_json,
            } => {
                self.relay_project_tree(&sid, &pid, tree_json).await?;
            }
            ClientMessage::RequestFile {
                session_id: sid,
                peer_id: pid,
                file_path,
            } => {
                self.forward_to_host(&sid, ServerMessage::RequestFile {
                    session_id: sid.clone(),
                    from_peer_id: pid,
                    file_path,
                }).await?;
            }
            ClientMessage::FileChunk {
                session_id: sid,
                peer_id: pid,
                file_path,
                offset,
                data,
                is_last,
            } => {
                self.relay_file_chunk(&sid, &pid, file_path, offset, data, is_last).await?;
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
        tx: mpsc::Sender<ServerMessage>,
        peer_id: &mut Option<String>,
        session_id: &mut Option<String>,
    ) -> Result<()> {
        // Validate join token (simplified implementation)
        // Production: Should use JWT verification with proper signing and expiration
        // For now: Token format validation only

        // If session doesn't exist, create it (first peer is host)
        let is_new_session = !self.sessions.contains_key(&sid);
        if is_new_session {
            self.create_session(sid.clone(), pid.clone())?;
        }

        let session = self
            .sessions
            .get(&sid)
            .context("Session not found")?
            .clone();

        // Add the new peer FIRST
        let peer = PeerSession::new(pid.clone(), sid.clone(), tx.clone());
        session.add_peer(peer);
        *peer_id = Some(pid.clone());
        *session_id = Some(sid.clone());

        info!(session = %sid, peer = %pid, "Peer joined session");

        // Get list of current participants AFTER adding new peer (so they're included)
        let participants: Vec<String> = session
            .list_peers()
            .iter()
            .map(|p| p.peer_id.clone())
            .collect();

        info!(session = %sid, "Sending Joined with {} participants: {:?}", participants.len(), participants);

        // Send Joined confirmation to the new peer
        tx.send(ServerMessage::Joined {
            session_id: sid.clone(),
            peer_id: pid.clone(),
            participants,
        })
        .await?;

        // Notify other peers about the new peer
        self.broadcast_peer_joined(&sid, &pid).await?;

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

    async fn handle_kick(&self, sid: &str, pid: &str, target_peer_id: String) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        // Verify the kicker is the host (first participant)
        let is_host = session
            .list_peers()
            .first()
            .map(|p| p.peer_id == pid)
            .unwrap_or(false);

        if !is_host {
            warn!(
                session = %sid,
                kicker = %pid,
                target = %target_peer_id,
                "Non-host attempted to kick user"
            );
            return Ok(());
        }

        // Get the target peer
        if let Some(target_peer) = session.get_peer(&target_peer_id) {
            info!(
                session = %sid,
                host = %pid,
                kicked_user = %target_peer_id,
                "Host kicked user from session"
            );

            // Send Kicked message to the target user
            let _ = target_peer
                .tx
                .send(ServerMessage::Kicked {
                    session_id: sid.to_string(),
                    reason: "Kicked by host".to_string(),
                })
                .await;

            // Remove the kicked user
            session.remove_peer(&target_peer_id);

            // Notify other peers that this user left
            self.broadcast_peer_left(sid, &target_peer_id).await?;

            METRICS
                .signaling_messages
                .with_label_values(&["kick"])
                .inc();
        } else {
            warn!(
                session = %sid,
                target = %target_peer_id,
                "Attempted to kick non-existent user"
            );
        }

        Ok(())
    }

    async fn relay_chat_message(&self, sid: &str, pid: &str, message: String) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let msg = ServerMessage::ChatMessage {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
            message,
            timestamp,
        };

        // Broadcast to all peers in session
        for peer in session.list_peers() {
            let _ = peer.tx.send(msg.clone()).await;
        }

        METRICS
            .signaling_messages
            .with_label_values(&["chat"])
            .inc();

        Ok(())
    }

    async fn forward_to_host(&self, sid: &str, msg: ServerMessage) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;
        let host = session
            .get_peer(&session.host_id)
            .context("Host not found")?;

        host.tx
            .send(msg)
            .await
            .context("Failed to send message to host")?;

        METRICS
            .signaling_messages
            .with_label_values(&["forwarded_to_host"])
            .inc();

        Ok(())
    }

    async fn relay_file_manifest(&self, sid: &str, pid: &str, manifest_json: String) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::FileManifest {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
            manifest_json,
        };

        // Send to all other peers (typically just the requesting guest)
        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["file_manifest"])
            .inc();

        Ok(())
    }

    async fn relay_files_chunk(
        &self,
        sid: &str,
        pid: &str,
        files_json: String,
        chunk_index: usize,
        total_chunks: usize,
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::FilesChunk {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
            files_json,
            chunk_index,
            total_chunks,
        };

        // Send to all other peers
        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["files_chunk"])
            .inc();

        Ok(())
    }

    async fn broadcast_peer_joined(&self, sid: &str, pid: &str) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::PeerJoined {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["peer_joined"])
            .inc();

        Ok(())
    }

    async fn broadcast_peer_left(&self, sid: &str, pid: &str) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::PeerLeft {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["peer_left"])
            .inc();

        Ok(())
    }

    async fn relay_p2p_request(
        &self,
        sid: &str,
        pid: &str,
        public_ip: String,
        public_port: u16,
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::P2PConnectionRequest {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
            public_ip,
            public_port,
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["p2p_request"])
            .inc();

        Ok(())
    }

    async fn relay_p2p_response(
        &self,
        sid: &str,
        pid: &str,
        public_ip: String,
        public_port: u16,
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::P2PConnectionResponse {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
            public_ip,
            public_port,
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["p2p_response"])
            .inc();

        Ok(())
    }

    async fn relay_binary_proxy_request(&self, sid: &str, pid: &str) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::RequestBinaryProxy {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["binary_proxy_request"])
            .inc();

        Ok(())
    }

    async fn relay_binary_proxy_data(
        &self,
        sid: &str,
        pid: &str,
        sequence: u64,
        is_git_protocol: bool,
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::BinaryProxyData {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
            sequence,
            is_git_protocol,
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["binary_proxy_data"])
            .inc();

        Ok(())
    }

    async fn relay_project_tree(&self, sid: &str, pid: &str, tree_json: String) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::ProjectTreeResponse {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
            tree_json,
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["project_tree"])
            .inc();

        Ok(())
    }

    async fn relay_file_chunk(
        &self,
        sid: &str,
        pid: &str,
        file_path: String,
        offset: u64,
        data: Vec<u8>,
        is_last: bool,
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::FileChunk {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
            file_path,
            offset,
            data,
            is_last,
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                let _ = peer.tx.send(msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["file_chunk"])
            .inc();

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
