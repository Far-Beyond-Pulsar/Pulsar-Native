use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use tracing::info;

mod link;
use link::{LinkShared, RelayTask, StudioTask};

/// Wire-level message for the `pulsar-studio` WebSocket session protocol.
///
/// Mirrors `studio-web/src/sessions/types.rs::WsMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StudioWsMessage {
    Ping,
    Pong,
    UserJoined {
        user: String,
    },
    UserLeft {
        user: String,
    },
    UserList {
        users: Vec<String>,
    },
    StatePatch {
        #[serde(flatten)]
        patch: serde_json::Value,
    },
    Chat {
        user: String,
        text: String,
    },
    Error {
        message: String,
    },
    FileChanged {
        path: String,
        kind: String,
    },
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

/// The WebSocket URL of a Studio workspace session. It carries the auth
/// token, so it is never logged.
fn studio_session_url(
    server_url: &str,
    workspace_id: &str,
    username: &str,
    auth_token: &str,
    environment_id: Option<&str>,
) -> String {
    let server_url_clean = server_url.trim().trim_end_matches('/');
    let ws_base = if let Some(rest) = server_url_clean.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = server_url_clean.strip_prefix("https://") {
        format!("wss://{rest}")
    } else {
        format!("ws://{server_url_clean}")
    };
    // Use per-environment endpoint when available for proper dashboard presence;
    // fall back to the legacy lobby endpoint.
    if let Some(eid) = environment_id {
        format!(
            "{}/api/v1/workspaces/{}/environments/{}/session?user={}&token={}",
            ws_base, workspace_id, eid, username, auth_token
        )
    } else {
        format!(
            "{}/api/v1/workspaces/{}/session?user={}&token={}",
            ws_base, workspace_id, username, auth_token
        )
    }
}

/// The channels of one connection attempt (see `MultiuserClient::open_channels`).
struct LinkChannels {
    message_tx: mpsc::UnboundedSender<ClientMessage>,
    message_rx: mpsc::UnboundedReceiver<ClientMessage>,
    message_tx_for_client: mpsc::UnboundedSender<ClientMessage>,
    event_tx: mpsc::UnboundedSender<ServerMessage>,
    event_rx: mpsc::UnboundedReceiver<ServerMessage>,
    shutdown_rx: watch::Receiver<bool>,
    result_tx: oneshot::Sender<Result<()>>,
    result_rx: oneshot::Receiver<Result<()>>,
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

        let channels = self.open_channels();

        // Spawn the entire connection process on Tokio runtime
        tokio_runtime().spawn(
            RelayTask {
                ws_url,
                shared: self.link_shared(),
                session_id,
                peer_id,
                join_token,
                identity,
                message_tx: channels.message_tx,
                message_rx: channels.message_rx,
                event_tx: channels.event_tx,
                shutdown_rx: channels.shutdown_rx,
                result_tx: channels.result_tx,
            }
            .run(),
        );

        self.await_connection(
            channels.result_rx,
            channels.message_tx_for_client,
            channels.event_rx,
        )
        .await
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
        let ws_url = studio_session_url(
            &self.server_url,
            &workspace_id,
            &username,
            &auth_token,
            environment_id.as_deref(),
        );

        // The URL carries the auth token, so it is never logged.
        info!("🔗 Connecting to Studio workspace session {}", workspace_id);

        let channels = self.open_channels();

        tokio_runtime().spawn(
            StudioTask {
                ws_url,
                shared: self.link_shared(),
                session_id: workspace_id,
                peer_id,
                username,
                message_tx: channels.message_tx,
                message_rx: channels.message_rx,
                event_tx: channels.event_tx,
                shutdown_rx: channels.shutdown_rx,
                result_tx: channels.result_tx,
            }
            .run(),
        );

        self.await_connection(
            channels.result_rx,
            channels.message_tx_for_client,
            channels.event_rx,
        )
        .await
    }

    /// The state a connection's tasks update.
    fn link_shared(&self) -> LinkShared {
        LinkShared {
            status: self.status.clone(),
            latency_ms: self.latency_ms.clone(),
            last_ping_at: self.last_ping_at.clone(),
        }
    }

    /// Create the channels of a new connection (bidirectional communication
    /// plus the shutdown signal and the handshake result) and keep the
    /// shutdown sender.
    fn open_channels(&mut self) -> LinkChannels {
        let (message_tx, message_rx) = mpsc::unbounded_channel::<ClientMessage>();
        let message_tx_for_client = message_tx.clone();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Use oneshot channel to get connection result from Tokio runtime
        let (result_tx, result_rx) = oneshot::channel();

        LinkChannels {
            message_tx,
            message_rx,
            message_tx_for_client,
            event_tx,
            event_rx,
            shutdown_rx,
            result_tx,
            result_rx,
        }
    }

    /// Wait for connection result from the Tokio runtime.
    async fn await_connection(
        &mut self,
        result_rx: oneshot::Receiver<Result<()>>,
        message_tx_for_client: mpsc::UnboundedSender<ClientMessage>,
        event_rx: mpsc::UnboundedReceiver<ServerMessage>,
    ) -> Result<mpsc::UnboundedReceiver<ServerMessage>> {
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
        tokio_runtime().block_on(self.connect_to_workspace(
            workspace_id,
            auth_token,
            username,
            environment_id,
        ))
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
