use anyhow::{Context, Result};
use async_tungstenite::{tokio::connect_async, tungstenite::Message};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use tracing::{debug, error, info, warn};

/// Wire-level message for the `pulsar-studio` WebSocket session protocol.
///
/// Mirrors `studio-web/src/sessions/types.rs::WsMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StudioWsMessage {
    Ping,
    Pong,
    UserJoined { user: String },
    UserLeft { user: String },
    UserList { users: Vec<String> },
    StatePatch { #[serde(flatten)] patch: serde_json::Value },
    Chat { user: String, text: String },
    Error { message: String },
    FileChanged { path: String, kind: String },
}

// Global Tokio runtime for WebSocket operations
fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("multiuser-ws")
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime for multiuser client")
    })
}

fn http_base_url(server_url: &str) -> String {
    let trimmed = server_url.trim().trim_end_matches('/');
    let base = if let Some(rest) = trimmed.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("wss://") {
        format!("https://{rest}")
    } else {
        trimmed.to_string()
    };

    base.trim_end_matches("/ws").to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PeerProfile {
    pub peer_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub github_login: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PeerIdentity {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub github_login: Option<String>,
}

/// Messages sent from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        session_id: String,
        peer_id: String,
        join_token: String,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        avatar_url: Option<String>,
        #[serde(default)]
        github_login: Option<String>,
    },
    Leave {
        session_id: String,
        peer_id: String,
    },
    KickUser {
        session_id: String,
        peer_id: String,        // Host's peer ID
        target_peer_id: String, // User to kick
    },
    ChatMessage {
        session_id: String,
        peer_id: String,
        message: String,
    },
    // P2P connection negotiation
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
    // Binary proxy mode (raw bytes, no JSON)
    // Note: This needs special handling - WebSocket sends as binary frame
    RequestBinaryProxy {
        session_id: String,
        peer_id: String,
    },
    BinaryProxyData {
        session_id: String,
        peer_id: String,
        // Data sent separately as WebSocket binary frame
        sequence: u64,
        is_git_protocol: bool,
    },
    // Git sync messages (JSON fallback)
    RequestProjectTree {
        session_id: String,
        peer_id: String,
    },
    ProjectTreeResponse {
        session_id: String,
        peer_id: String,
        tree_json: String, // Commit hash or full tree
    },
    // Simple file sync messages
    RequestFileManifest {
        session_id: String,
        peer_id: String,
    },
    FileManifest {
        session_id: String,
        peer_id: String,
        manifest_json: String, // serialized simple_sync::FileManifest
    },
    RequestFiles {
        session_id: String,
        peer_id: String,
        file_paths: Vec<String>, // List of files needed
    },
    FilesChunk {
        session_id: String,
        peer_id: String,
        files_json: String, // serialized Vec<(String, Vec<u8>)>
        chunk_index: usize,
        total_chunks: usize,
    },
    FileChanged {
        session_id: String,
        peer_id: String,
        path: String,
        kind: String,
    },
    // Legacy file-based messages (kept for backwards compatibility)
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
    // UI replication messages for real-time collaborative editing
    ReplicationUpdate {
        session_id: String,
        peer_id: String,
        data: String, // serialized ReplicationMessage
    },
    Ping,
}

/// Messages received from server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Joined {
        session_id: String,
        peer_id: String,
        participants: Vec<String>,
        #[serde(default)]
        join_token: Option<String>,
        #[serde(default)]
        participant_profiles: Option<Vec<PeerProfile>>,
    },
    PeerJoined {
        session_id: String,
        peer_id: String,
        #[serde(default)]
        profile: Option<PeerProfile>,
    },
    PeerLeft {
        session_id: String,
        peer_id: String,
    },
    Kicked {
        session_id: String,
        reason: String,
    },
    ChatMessage {
        session_id: String,
        peer_id: String,
        message: String,
        timestamp: u64,
    },
    // P2P connection negotiation (relayed)
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
    // Binary proxy (relayed)
    RequestBinaryProxy {
        session_id: String,
        from_peer_id: String,
    },
    BinaryProxyData {
        session_id: String,
        from_peer_id: String,
        sequence: u64,
        is_git_protocol: bool,
        // Binary data comes in separate WebSocket frame
    },
    // Git sync messages (JSON fallback, relayed)
    RequestProjectTree {
        session_id: String,
        from_peer_id: String,
    },
    ProjectTreeResponse {
        session_id: String,
        from_peer_id: String,
        tree_json: String,
    },
    // Simple file sync messages
    RequestFileManifest {
        session_id: String,
        from_peer_id: String,
    },
    FileManifest {
        session_id: String,
        from_peer_id: String,
        manifest_json: String,
    },
    RequestFiles {
        session_id: String,
        from_peer_id: String,
        file_paths: Vec<String>,
    },
    FilesChunk {
        session_id: String,
        from_peer_id: String,
        files_json: String,
        chunk_index: usize,
        total_chunks: usize,
    },
    FileChanged {
        session_id: String,
        from_peer_id: String,
        path: String,
        kind: String,
    },
    // Legacy file messages
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
    // UI replication messages for real-time collaborative editing
    ReplicationUpdate {
        session_id: String,
        from_peer_id: String,
        data: String, // serialized ReplicationMessage
    },
    Pong,
    Error {
        message: String,
    },
}

/// Connection status
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// Multiuser client for connecting to collaboration server
pub struct MultiuserClient {
    server_url: String,
    status: Arc<RwLock<ConnectionStatus>>,
    message_tx: Option<mpsc::UnboundedSender<ClientMessage>>,
    event_rx: Option<mpsc::UnboundedReceiver<ServerMessage>>,
    latency_ms: Arc<RwLock<Option<u32>>>,
    last_ping_at: Arc<RwLock<Option<Instant>>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl MultiuserClient {
    pub fn new(server_url: String) -> Self {
        Self {
            server_url,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            message_tx: None,
            event_rx: None,
            latency_ms: Arc::new(RwLock::new(None)),
            last_ping_at: Arc::new(RwLock::new(None)),
            shutdown_tx: None,
        }
    }

    /// Get current connection status
    pub async fn status(&self) -> ConnectionStatus {
        self.status.read().await.clone()
    }

    pub async fn latency_ms(&self) -> Option<u32> {
        *self.latency_ms.read().await
    }

    /// Create a new session (generates local credentials)
    /// The server creates the session via HTTP and returns the join token.
    pub async fn create_session(&self) -> Result<(String, String)> {
        let base_url = http_base_url(&self.server_url);
        let host_id = uuid::Uuid::new_v4().to_string();
        let request_url = format!("{}/v1/sessions", base_url);

        info!("Creating session via HTTP: {}", request_url);

        let response = reqwest::blocking::Client::builder()
            .build()
            .context("Failed to build create-session client")?
            .post(&request_url)
            .json(&serde_json::json!({
                "host_id": host_id,
                "metadata": {},
            }))
            .send()
            .context("Failed to send create-session request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            anyhow::bail!("Create session failed: {} {}", status, body);
        }

        let payload: serde_json::Value = response
            .json()
            .context("Failed to decode create-session response")?;

        let session_id = payload
            .get("session_id")
            .and_then(|v| v.as_str())
            .context("Create-session response missing session_id")?
            .to_string();
        let join_token = payload
            .get("join_token")
            .and_then(|v| v.as_str())
            .context("Create-session response missing join_token")?
            .to_string();

        info!("Created session {} via server API", session_id);
        Ok((session_id, join_token))
    }

    /// Connect to a session via WebSocket
    pub async fn connect(
        &mut self,
        session_id: String,
        join_token: String,
        identity: Option<PeerIdentity>,
    ) -> Result<mpsc::UnboundedReceiver<ServerMessage>> {
        *self.status.write().await = ConnectionStatus::Connecting;
        *self.latency_ms.write().await = None;

        let peer_id = uuid::Uuid::new_v4().to_string();

        // Trim and sanitize server URL
        let server_url_clean = self.server_url.trim();
        let ws_url = format!("{}/ws", server_url_clean);

        info!("🔗 Server URL (raw): {:?}", self.server_url);
        info!("🔗 Server URL (clean): {:?}", server_url_clean);
        info!("🔗 WebSocket URL: {:?}", ws_url);
        info!("Connecting to WebSocket: {}", ws_url);

        // Create channels for bidirectional communication
        let (message_tx, message_rx) = mpsc::unbounded_channel::<ClientMessage>();
        let message_tx_for_client = message_tx.clone();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx.clone());

        // Use oneshot channel to get connection result from Tokio runtime
        let (result_tx, result_rx) = oneshot::channel();

        let status_clone = self.status.clone();
        let latency_ms_clone = self.latency_ms.clone();
        let last_ping_at_clone = self.last_ping_at.clone();
        let session_id_clone = session_id.clone();
        let peer_id_clone = peer_id.clone();

        // Spawn the entire connection process on Tokio runtime
        tokio_runtime().spawn(async move {
            // Connect to WebSocket (runs in Tokio runtime context)
            let connect_result = connect_async(&ws_url).await;

            match connect_result {
                Ok((ws_stream, _)) => {
                    info!("WebSocket connected successfully");

                    let (mut write, mut read) = ws_stream.split();

                    // Send initial Join message
                    let join_msg = ClientMessage::Join {
                        session_id: session_id_clone.clone(),
                        peer_id: peer_id_clone.clone(),
                        join_token,
                        display_name: identity.as_ref().and_then(|i| i.display_name.clone()),
                        avatar_url: identity.as_ref().and_then(|i| i.avatar_url.clone()),
                        github_login: identity.as_ref().and_then(|i| i.github_login.clone()),
                    };

                    if let Ok(join_json) = serde_json::to_string(&join_msg) {
                        if let Err(e) = write.send(Message::Text(join_json.into())).await {
                            error!("Failed to send join message: {}", e);
                            let _ =
                                result_tx.send(Err(anyhow::anyhow!("Failed to send join: {}", e)));
                            return;
                        }
                    }

                    // Signal successful connection
                    let _ = result_tx.send(Ok(()));

                    // Spawn task to handle outgoing messages
                    let status_clone_out = status_clone.clone();
                    let ping_tx = message_tx.clone();
                    let status_clone_ping = status_clone.clone();
                    let last_ping_at_for_ping = last_ping_at_clone.clone();
                    let mut write_shutdown_rx = shutdown_rx.clone();
                    tokio::spawn(async move {
                        let mut message_rx = message_rx;
                        loop {
                            tokio::select! {
                                changed = write_shutdown_rx.changed() => {
                                    if changed.is_ok() && *write_shutdown_rx.borrow() {
                                        let _ = write.send(Message::Close(None)).await;
                                        break;
                                    }
                                }
                                maybe_msg = message_rx.recv() => {
                                    match maybe_msg {
                                        Some(msg) => {
                                            if let Ok(json) = serde_json::to_string(&msg) {
                                                if let Err(e) = write.send(Message::Text(json.into())).await {
                                                    error!("Failed to send message: {}", e);
                                                    *status_clone_out.write().await =
                                                        ConnectionStatus::Error(e.to_string());
                                                    break;
                                                }
                                            }
                                        }
                                        None => {
                                            let _ = write.send(Message::Close(None)).await;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    });

                    let mut read_shutdown_rx = shutdown_rx.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::select! {
                                changed = read_shutdown_rx.changed() => {
                                    if changed.is_ok() && *read_shutdown_rx.borrow() {
                                        break;
                                    }
                                }
                                _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {
                                    if matches!(*status_clone_ping.read().await, ConnectionStatus::Disconnected) {
                                        break;
                                    }

                                    *last_ping_at_for_ping.write().await = Some(Instant::now());
                                    if ping_tx.send(ClientMessage::Ping).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    });

                    // Handle incoming messages
                    let mut incoming_shutdown_rx = shutdown_rx.clone();
                    loop {
                        tokio::select! {
                            changed = incoming_shutdown_rx.changed() => {
                                if changed.is_ok() && *incoming_shutdown_rx.borrow() {
                                    break;
                                }
                            }
                            result = read.next() => {
                                match result {
                                    Some(Ok(Message::Text(text))) => {
                                        match serde_json::from_str::<ServerMessage>(&text) {
                                            Ok(msg) => {
                                                debug!("Received message: {:?}", msg);

                                                if matches!(&msg, ServerMessage::Pong) {
                                                    if let Some(sent_at) = last_ping_at_clone.write().await.take() {
                                                        let rtt = sent_at.elapsed().as_millis();
                                                        *latency_ms_clone.write().await = Some(rtt as u32);
                                                    }
                                                }

                                                // Update status on successful join
                                                if matches!(msg, ServerMessage::Joined { .. }) {
                                                    *status_clone.write().await =
                                                        ConnectionStatus::Connected;
                                                }

                                                if event_tx.send(msg).is_err() {
                                                    warn!("Event receiver dropped");
                                                    break;
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to parse server message: {}", e);
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) => {
                                        info!("WebSocket closed by server");
                                        *status_clone.write().await = ConnectionStatus::Disconnected;
                                        *latency_ms_clone.write().await = None;
                                        break;
                                    }
                                    Some(Ok(Message::Ping(_))) => {
                                        // Pings are handled automatically by tungstenite
                                    }
                                    Some(Ok(_)) => {}
                                    Some(Err(e)) => {
                                        error!("WebSocket error: {}", e);
                                        *status_clone.write().await =
                                            ConnectionStatus::Error(e.to_string());
                                        *latency_ms_clone.write().await = None;
                                        break;
                                    }
                                    None => break,
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to WebSocket: {}", e);
                    *status_clone.write().await = ConnectionStatus::Error(e.to_string());
                    *latency_ms_clone.write().await = None;
                    let _ = result_tx.send(Err(anyhow::anyhow!("Connection failed: {}", e)));
                }
            }
        });

        // Wait for connection result from Tokio runtime
        match result_rx.await {
            Ok(Ok(())) => {
                self.message_tx = Some(message_tx_for_client);
                Ok(event_rx)
            }
            Ok(Err(e)) => {
                self.shutdown_tx = None;
                Err(e)
            }
            Err(_) => {
                self.shutdown_tx = None;
                Err(anyhow::anyhow!("Connection task failed"))
            }
        }
    }

    /// Connect to a `pulsar-studio` workspace session via Studio WebSocket protocol.
    ///
    /// Unlike [`connect`] which talks the pulsar-relay signalling protocol, this
    /// method connects to the Studio session endpoint at
    /// `/api/v1/workspaces/{wid}/session` and uses the simpler
    /// `StudioWsMessage` protocol (pub/sub with user-list, chat and
    /// state-patch messages).
    pub async fn connect_to_workspace(
        &mut self,
        workspace_id: String,
        auth_token: String,
        username: String,
        environment_id: Option<String>,
    ) -> Result<mpsc::UnboundedReceiver<ServerMessage>> {
        *self.status.write().await = ConnectionStatus::Connecting;
        *self.latency_ms.write().await = None;

        let peer_id = username.clone();

        // Build WebSocket URL for Studio session endpoint
        let server_url_clean = self.server_url.trim().trim_end_matches('/');
        let ws_base = if let Some(rest) = server_url_clean.strip_prefix("http://") {
            format!("ws://{rest}")
        } else if let Some(rest) = server_url_clean.strip_prefix("https://") {
            format!("wss://{rest}")
        } else {
            format!("ws://{server_url_clean}")
        };
        // Use per-environment endpoint when available for proper dashboard presence;
        // fall back to the legacy lobby endpoint.
        let ws_url = if let Some(ref eid) = environment_id {
            format!(
                "{}/api/v1/workspaces/{}/environments/{}/session?user={}&token={}",
                ws_base, workspace_id, eid, username, auth_token
            )
        } else {
            format!(
                "{}/api/v1/workspaces/{}/session?user={}&token={}",
                ws_base, workspace_id, username, auth_token
            )
        };

        info!("🔗 Connecting to Studio workspace session: {:?}", ws_url);

        // Create channels for bidirectional communication
        let (message_tx, message_rx) = mpsc::unbounded_channel::<ClientMessage>();
        let message_tx_for_client = message_tx.clone();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx.clone());

        let (result_tx, result_rx) = oneshot::channel();

        let status_clone = self.status.clone();
        let latency_ms_clone = self.latency_ms.clone();
        let last_ping_at_clone = self.last_ping_at.clone();
        let session_id_clone = workspace_id.clone();
        let peer_id_clone = peer_id.clone();

        tokio_runtime().spawn(async move {
            let connect_result = connect_async(&ws_url).await;

            match connect_result {
                Ok((ws_stream, _)) => {
                    info!("Studio WebSocket connected successfully");

                    let (mut write, mut read) = ws_stream.split();

                    // ── Send initial presence ──
                    // Studio protocol doesn't require an explicit Join message;
                    // just connecting with query params establishes identity.

                    // Signal successful connection
                    let _ = result_tx.send(Ok(()));

                    // Spawn task to handle outgoing messages (map ClientMessage → StudioWsMessage)
                    let status_clone_out = status_clone.clone();
                    let ping_tx = message_tx.clone();
                    let status_clone_ping = status_clone.clone();
                    let last_ping_at_for_ping = last_ping_at_clone.clone();
                    let mut write_shutdown_rx = shutdown_rx.clone();
                    let username_for_write = username.clone();
                    let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<StudioWsMessage>();
                    tokio::spawn(async move {
                        let mut message_rx = message_rx;
                        loop {
                            tokio::select! {
                                changed = write_shutdown_rx.changed() => {
                                    if changed.is_ok() && *write_shutdown_rx.borrow() {
                                        let _ = write.send(Message::Close(None)).await;
                                        break;
                                    }
                                }
                                maybe_raw = raw_rx.recv() => {
                                    match maybe_raw {
                                        Some(ws_msg) => {
                                            if let Ok(json) = serde_json::to_string(&ws_msg) {
                                                if let Err(e) = write.send(Message::Text(json.into())).await {
                                                    error!("Failed to send message: {}", e);
                                                    *status_clone_out.write().await =
                                                        ConnectionStatus::Error(e.to_string());
                                                    break;
                                                }
                                            }
                                        }
                                        None => {
                                            let _ = write.send(Message::Close(None)).await;
                                            break;
                                        }
                                    }
                                }
                                maybe_msg = message_rx.recv() => {
                                    match maybe_msg {
                                        Some(msg) => {
                                            let ws_msg = match msg {
                                                ClientMessage::ChatMessage { message, .. } => {
                                                    StudioWsMessage::Chat {
                                                        user: username_for_write.clone(),
                                                        text: message,
                                                    }
                                                }
                                                ClientMessage::Ping => StudioWsMessage::Ping,
                                                _ => continue,
                                            };
                                            if let Ok(json) = serde_json::to_string(&ws_msg) {
                                                if let Err(e) = write.send(Message::Text(json.into())).await {
                                                    error!("Failed to send message: {}", e);
                                                    *status_clone_out.write().await =
                                                        ConnectionStatus::Error(e.to_string());
                                                    break;
                                                }
                                            }
                                        }
                                        None => {
                                            let _ = write.send(Message::Close(None)).await;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    });

                    // Spawn ping task
                    let mut read_shutdown_rx = shutdown_rx.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::select! {
                                changed = read_shutdown_rx.changed() => {
                                    if changed.is_ok() && *read_shutdown_rx.borrow() {
                                        break;
                                    }
                                }
                                _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => {
                                    if matches!(*status_clone_ping.read().await, ConnectionStatus::Disconnected) {
                                        break;
                                    }
                                    *last_ping_at_for_ping.write().await = Some(Instant::now());
                                    if ping_tx.send(ClientMessage::Ping).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    });

                    // ── Handle incoming messages (StudioWsMessage → ServerMessage) ──
                    let mut incoming_shutdown_rx = shutdown_rx.clone();
                    let mut first_message = true;
                    loop {
                        tokio::select! {
                            changed = incoming_shutdown_rx.changed() => {
                                if changed.is_ok() && *incoming_shutdown_rx.borrow() {
                                    break;
                                }
                            }
                            result = read.next() => {
                                match result {
                                    Some(Ok(Message::Text(text))) => {
                                        match serde_json::from_str::<StudioWsMessage>(&text) {
                                            Ok(studio_msg) => {
                                                debug!("Received Studio message: {:?}", studio_msg);

                                                match studio_msg {
                                                    StudioWsMessage::Pong => {
                                                        if let Some(sent_at) = last_ping_at_clone.write().await.take() {
                                                            let rtt = sent_at.elapsed().as_millis();
                                                            *latency_ms_clone.write().await = Some(rtt as u32);
                                                        }
                                                    }
                                                    StudioWsMessage::UserList { users } => {
                                                        if first_message {
                                                            first_message = false;
                                                            *status_clone.write().await =
                                                                ConnectionStatus::Connected;
                                                            let _ = event_tx.send(ServerMessage::Joined {
                                                                session_id: session_id_clone.clone(),
                                                                peer_id: peer_id_clone.clone(),
                                                                participants: users,
                                                                join_token: None,
                                                                participant_profiles: None,
                                                            });
                                                        }
                                                    }
                                                    StudioWsMessage::UserJoined { user } => {
                                                        let _ = event_tx.send(ServerMessage::PeerJoined {
                                                            session_id: session_id_clone.clone(),
                                                            peer_id: user,
                                                            profile: None,
                                                        });
                                                    }
                                                    StudioWsMessage::UserLeft { user } => {
                                                        let _ = event_tx.send(ServerMessage::PeerLeft {
                                                            session_id: session_id_clone.clone(),
                                                            peer_id: user,
                                                        });
                                                    }
                                                    StudioWsMessage::Chat { user, text } => {
                                                        let _ = event_tx.send(ServerMessage::ChatMessage {
                                                            session_id: session_id_clone.clone(),
                                                            peer_id: user,
                                                            message: text,
                                                            timestamp: std::time::SystemTime::now()
                                                                .duration_since(std::time::UNIX_EPOCH)
                                                                .map(|d| d.as_secs())
                                                                .unwrap_or(0),
                                                        });
                                                    }
                                                    StudioWsMessage::StatePatch { patch } => {
                                                        let data = patch.to_string();
                                                        let _ = event_tx.send(ServerMessage::ReplicationUpdate {
                                                            session_id: session_id_clone.clone(),
                                                            from_peer_id: "studio".to_string(),
                                                            data,
                                                        });
                                                    }
                                                    StudioWsMessage::Error { message } => {
                                                        error!("Studio session error: {}", message);
                                                        *status_clone.write().await =
                                                            ConnectionStatus::Error(message.clone());
                                                        let _ = event_tx.send(ServerMessage::Error { message });
                                                        break;
                                                    }
                                                    StudioWsMessage::Ping => {
                                                        // Respond with Pong
                                                        let _ = raw_tx.send(StudioWsMessage::Pong);
                                                    }
                                                    StudioWsMessage::FileChanged { .. } => {
                                                        // File changes are handled via HTTP file API; ignore for now
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to parse Studio message: {}", e);
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) => {
                                        info!("Studio WebSocket closed by server");
                                        *status_clone.write().await = ConnectionStatus::Disconnected;
                                        *latency_ms_clone.write().await = None;
                                        break;
                                    }
                                    Some(Ok(Message::Ping(_))) => {}
                                    Some(Ok(_)) => {}
                                    Some(Err(e)) => {
                                        error!("Studio WebSocket error: {}", e);
                                        *status_clone.write().await =
                                            ConnectionStatus::Error(e.to_string());
                                        *latency_ms_clone.write().await = None;
                                        break;
                                    }
                                    None => break,
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to Studio WebSocket: {}", e);
                    *status_clone.write().await = ConnectionStatus::Error(e.to_string());
                    *latency_ms_clone.write().await = None;
                    let _ = result_tx.send(Err(anyhow::anyhow!("Connection failed: {}", e)));
                }
            }
        });

        // Wait for connection result from Tokio runtime
        match result_rx.await {
            Ok(Ok(())) => {
                self.message_tx = Some(message_tx_for_client);
                Ok(event_rx)
            }
            Ok(Err(e)) => {
                self.shutdown_tx = None;
                Err(e)
            }
            Err(_) => {
                self.shutdown_tx = None;
                Err(anyhow::anyhow!("Connection task failed"))
            }
        }
    }

    /// Synchronous wrapper around [`connect_to_workspace`] that blocks on the global
    /// Tokio runtime.  Lets callers from non-Tokio contexts (e.g. gpui background
    /// threads) drive the async handshake without depending on tokio directly.
    /// If `environment_id` is provided, connects to the per-environment session
    /// endpoint for proper dashboard presence tracking.
    pub fn connect_to_workspace_sync(
        &mut self,
        workspace_id: String,
        auth_token: String,
        username: String,
        environment_id: Option<String>,
    ) -> Result<mpsc::UnboundedReceiver<ServerMessage>> {
        tokio_runtime().block_on(self.connect_to_workspace(workspace_id, auth_token, username, environment_id))
    }

    /// Send a message to the server
    pub async fn send(&self, message: ClientMessage) -> Result<()> {
        if let Some(tx) = &self.message_tx {
            tx.send(message).context("Failed to send message")?;
            Ok(())
        } else {
            anyhow::bail!("Not connected")
        }
    }

    /// Disconnect from the session
    pub async fn disconnect(&mut self, session_id: String, peer_id: String) -> Result<()> {
        if let Some(tx) = &self.message_tx {
            let leave_msg = ClientMessage::Leave {
                session_id,
                peer_id,
            };
            tx.send(leave_msg)?;
        }

        if let Some(shutdown_tx) = &self.shutdown_tx {
            let _ = shutdown_tx.send(true);
        }
        self.message_tx = None;
        self.shutdown_tx = None;
        *self.status.write().await = ConnectionStatus::Disconnected;
        *self.latency_ms.write().await = None;

        Ok(())
    }

    /// Forcefully drop the client without sending a Leave message.
    pub async fn force_disconnect(&mut self) {
        if let Some(shutdown_tx) = &self.shutdown_tx {
            let _ = shutdown_tx.send(true);
        }
        self.message_tx = None;
        self.event_rx = None;
        self.shutdown_tx = None;
        *self.status.write().await = ConnectionStatus::Disconnected;
        *self.latency_ms.write().await = None;
        *self.last_ping_at.write().await = None;
    }
}
