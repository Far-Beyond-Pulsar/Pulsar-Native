//! WebSocket signaling coordinator - orchestration entry point.
//!
//! This module wires together protocol types, peer state, and session lifecycle.

mod peer_discovery;
mod session_manager;
mod sync_protocol;

pub use pulsar_multiplayer_core::session::PeerProfile;
pub use session_manager::RendezvousCoordinator;
pub use sync_protocol::{CandidateDto, ClientMessage, ServerMessage};

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use pulsar_multiplayer_core::session::Role;

use crate::metrics::METRICS;
use peer_discovery::PeerSession;

impl RendezvousCoordinator {
    /// How long a broadcast waits on one peer's outbound channel. The
    /// channel is bounded; a peer that stopped reading its socket fills it,
    /// and waiting on it forever would stall the sending peer's handler, and
    /// with it everything that peer relays, for as long as the stuck peer
    /// stays connected.
    const SEND_TIMEOUT: Duration = Duration::from_secs(2);

    /// Queue `msg` for `peer`, giving up after [`Self::SEND_TIMEOUT`].
    async fn deliver(peer: &PeerSession, msg: ServerMessage) {
        match tokio::time::timeout(Self::SEND_TIMEOUT, peer.tx.send(msg)).await {
            Ok(Ok(())) => {}
            // The receiver is gone: the peer's connection is closing.
            Ok(Err(_)) => {}
            Err(_) => warn!(
                peer = %peer.peer_id,
                session = %peer.session_id,
                "Peer is not reading its messages; dropped a broadcast to it"
            ),
        }
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

                if sender.send(Message::Text(json.into())).await.is_err() {
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
                            let result = self
                                .handle_client_message(
                                    client_msg,
                                    tx.clone(),
                                    &mut peer_id,
                                    &mut session_id,
                                )
                                .await;

                            if let Err(e) = result {
                                error!(error = %e, "Failed to handle client message");
                                let _ = tx
                                    .send(ServerMessage::Error {
                                        message: e.to_string(),
                                    })
                                    .await;
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

    /// The session and peer a message says it comes from: `None` for the
    /// messages that carry neither (`Join` establishes them, `Ping` needs
    /// none).
    fn claimed_identity(msg: &ClientMessage) -> Option<(&str, &str)> {
        use ClientMessage::*;
        match msg {
            Join { .. } | Ping => None,
            Leave {
                session_id,
                peer_id,
            }
            | KickUser {
                session_id,
                peer_id,
                ..
            }
            | ChatMessage {
                session_id,
                peer_id,
                ..
            }
            | RequestFileManifest {
                session_id,
                peer_id,
            }
            | FileManifest {
                session_id,
                peer_id,
                ..
            }
            | RequestFiles {
                session_id,
                peer_id,
                ..
            }
            | FilesChunk {
                session_id,
                peer_id,
                ..
            }
            | FileChanged {
                session_id,
                peer_id,
                ..
            }
            | P2PConnectionRequest {
                session_id,
                peer_id,
                ..
            }
            | P2PConnectionResponse {
                session_id,
                peer_id,
                ..
            }
            | RequestBinaryProxy {
                session_id,
                peer_id,
            }
            | BinaryProxyData {
                session_id,
                peer_id,
                ..
            }
            | RequestProjectTree {
                session_id,
                peer_id,
            }
            | ProjectTreeResponse {
                session_id,
                peer_id,
                ..
            }
            | RequestFile {
                session_id,
                peer_id,
                ..
            }
            | FileChunk {
                session_id,
                peer_id,
                ..
            } => Some((session_id.as_str(), peer_id.as_str())),
        }
    }

    async fn handle_client_message(
        &self,
        msg: ClientMessage,
        tx: mpsc::Sender<ServerMessage>,
        peer_id: &mut Option<String>,
        session_id: &mut Option<String>,
    ) -> Result<()> {
        // The identity a message carries is only ever the one this connection
        // joined with. The handlers below trust the message's `peer_id` and
        // `session_id` (relaying as that peer, leaving or kicking as it), so
        // a guest naming the host's id here would otherwise speak for the
        // host, and a connection that never joined could act in any session.
        if let Some((claimed_session, claimed_peer)) = Self::claimed_identity(&msg) {
            if session_id.as_deref() != Some(claimed_session)
                || peer_id.as_deref() != Some(claimed_peer)
            {
                anyhow::bail!(
                    "message from peer {claimed_peer} in session {claimed_session} on a connection that joined as {peer_id:?} in {session_id:?}"
                );
            }
        }

        match msg {
            ClientMessage::Join {
                session_id: sid,
                peer_id: pid,
                join_token,
                display_name,
                avatar_url,
                github_login,
            } => {
                self.handle_join(
                    sid,
                    pid,
                    join_token,
                    display_name,
                    avatar_url,
                    github_login,
                    tx,
                    peer_id,
                    session_id,
                )
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
                self.forward_to_host(
                    &sid,
                    ServerMessage::RequestFileManifest {
                        session_id: sid.clone(),
                        from_peer_id: pid,
                    },
                )
                .await?;
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
                self.forward_to_host(
                    &sid,
                    ServerMessage::RequestFiles {
                        session_id: sid.clone(),
                        from_peer_id: pid,
                        file_paths,
                    },
                )
                .await?;
            }
            ClientMessage::FilesChunk {
                session_id: sid,
                peer_id: pid,
                files_json,
                chunk_index,
                total_chunks,
            } => {
                self.relay_files_chunk(&sid, &pid, files_json, chunk_index, total_chunks)
                    .await?;
            }
            ClientMessage::FileChanged {
                session_id: sid,
                peer_id: pid,
                path,
                kind,
            } => {
                self.relay_file_changed(&sid, &pid, path, kind).await?;
            }
            ClientMessage::P2PConnectionRequest {
                session_id: sid,
                peer_id: pid,
                public_ip,
                public_port,
            } => {
                self.relay_p2p_request(&sid, &pid, public_ip, public_port)
                    .await?;
            }
            ClientMessage::P2PConnectionResponse {
                session_id: sid,
                peer_id: pid,
                public_ip,
                public_port,
            } => {
                self.relay_p2p_response(&sid, &pid, public_ip, public_port)
                    .await?;
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
                self.relay_binary_proxy_data(&sid, &pid, sequence, is_git_protocol)
                    .await?;
            }
            ClientMessage::RequestProjectTree {
                session_id: sid,
                peer_id: pid,
            } => {
                self.forward_to_host(
                    &sid,
                    ServerMessage::RequestProjectTree {
                        session_id: sid.clone(),
                        from_peer_id: pid,
                    },
                )
                .await?;
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
                self.forward_to_host(
                    &sid,
                    ServerMessage::RequestFile {
                        session_id: sid.clone(),
                        from_peer_id: pid,
                        file_path,
                    },
                )
                .await?;
            }
            ClientMessage::FileChunk {
                session_id: sid,
                peer_id: pid,
                file_path,
                offset,
                data,
                is_last,
            } => {
                self.relay_file_chunk(&sid, &pid, file_path, offset, data, is_last)
                    .await?;
            }
            ClientMessage::Ping => {
                tx.send(ServerMessage::Pong).await?;
            }
        }

        Ok(())
    }

    async fn handle_join(
        &self,
        mut sid: String,
        pid: String,
        join_token: String,
        display_name: Option<String>,
        avatar_url: Option<String>,
        github_login: Option<String>,
        tx: mpsc::Sender<ServerMessage>,
        peer_id: &mut Option<String>,
        session_id: &mut Option<String>,
    ) -> Result<()> {
        if peer_id.is_some() || session_id.is_some() {
            tx.send(ServerMessage::Error {
                message: "This connection already joined a session".to_string(),
            })
            .await?;
            anyhow::bail!("connection joined as {peer_id:?} in {session_id:?} sent a second Join");
        }

        let creating_session = join_token.trim().is_empty();
        let mut created_join_token = None;

        if creating_session {
            info!("No join token provided; creating a new session");
            if sid.trim().is_empty() {
                sid = Uuid::new_v4().to_string()[..8].to_string();
            }
            // Only a session that does not exist yet is created this way.
            // Joining an existing one takes its join token; without this
            // check anyone who learned a session id walked in as a second
            // "host" and was handed a host token for it.
            if self.sessions.contains_key(&sid) {
                tx.send(ServerMessage::Error {
                    message: "Session already exists; a join token is required".to_string(),
                })
                .await?;
                anyhow::bail!("refused to join existing session {sid} without a join token");
            }
            self.create_session(sid.clone(), pid.clone())?;
            created_join_token = Some(self.auth.create_join_token(
                sid.clone(),
                Role::Host,
                Duration::from_secs(3600),
            )?);
        } else {
            let (token_session, _role) = match self.auth.verify_join_token(&join_token) {
                Ok(verified) => verified,
                Err(e) => {
                    error!(error = %e, "Invalid join token");
                    tx.send(ServerMessage::Error {
                        message: "Invalid join token".to_string(),
                    })
                    .await?;
                    return Err(e);
                }
            };
            // A token admits its holder to the session it was minted for,
            // not to whichever session the message names.
            if token_session != sid {
                tx.send(ServerMessage::Error {
                    message: "Join token is for another session".to_string(),
                })
                .await?;
                anyhow::bail!("join token for session {token_session} used on session {sid}");
            }
            // A session created over the REST API has no rendezvous entry
            // until its host connects; the token proves this relay issued it
            // for `sid`, so the first peer in becomes the host.
            if !self.sessions.contains_key(&sid) {
                self.create_session(sid.clone(), pid.clone())?;
            }
        }

        let session = self
            .sessions
            .get(&sid)
            .context("Session not found")?
            .clone();

        // A peer id names one connection. Admitting a second one under the
        // host's id would replace the host's channel in the session and hand
        // the newcomer everything routed to the host.
        if session.get_peer(&pid).is_some() {
            tx.send(ServerMessage::Error {
                message: "Peer id is already in the session".to_string(),
            })
            .await?;
            anyhow::bail!("peer {pid} is already in session {sid}");
        }

        // Add the new peer FIRST
        let profile = if display_name.is_none() && avatar_url.is_none() && github_login.is_none() {
            None
        } else {
            Some(PeerProfile {
                peer_id: pid.clone(),
                display_name,
                avatar_url,
                github_login,
            })
        };
        let peer = PeerSession::new(pid.clone(), sid.clone(), tx.clone(), profile.clone());
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
        let participant_profiles: Vec<PeerProfile> = session
            .list_peers()
            .iter()
            .filter_map(|p| p.profile.clone())
            .collect();

        info!(session = %sid, "Sending Joined with {} participants: {:?}", participants.len(), participants);

        // Send Joined confirmation to the new peer
        tx.send(ServerMessage::Joined {
            session_id: sid.clone(),
            peer_id: pid.clone(),
            participants,
            join_token: created_join_token,
            participant_profiles: if participant_profiles.is_empty() {
                None
            } else {
                Some(participant_profiles)
            },
        })
        .await?;

        // Notify other peers about the new peer
        self.broadcast_peer_joined(&sid, &pid, profile).await?;

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

        // Verify the kicker is the host. The session records who created it;
        // the peer map's iteration order says nothing about that.
        let is_host = session.host_id == pid;

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
            .map_or(0, |d| d.as_secs());

        let msg = ServerMessage::ChatMessage {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
            message,
            timestamp,
        };

        // Broadcast to all peers in session
        for peer in session.list_peers() {
            Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
            }
        }

        METRICS
            .signaling_messages
            .with_label_values(&["files_chunk"])
            .inc();

        Ok(())
    }

    async fn relay_file_changed(
        &self,
        sid: &str,
        pid: &str,
        path: String,
        kind: String,
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::FileChanged {
            session_id: sid.to_string(),
            from_peer_id: pid.to_string(),
            path,
            kind,
        };

        for peer in session.list_peers() {
            Self::deliver(&peer, msg.clone()).await;
        }

        METRICS
            .signaling_messages
            .with_label_values(&["file_changed"])
            .inc();

        Ok(())
    }

    async fn broadcast_peer_joined(
        &self,
        sid: &str,
        pid: &str,
        profile: Option<PeerProfile>,
    ) -> Result<()> {
        let session = self.sessions.get(sid).context("Session not found")?;

        let msg = ServerMessage::PeerJoined {
            session_id: sid.to_string(),
            peer_id: pid.to_string(),
            profile,
        };

        for peer in session.list_peers() {
            if peer.peer_id != pid {
                Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
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
                Self::deliver(&peer, msg.clone()).await;
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
