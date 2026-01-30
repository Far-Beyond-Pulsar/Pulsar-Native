use anyhow::{Context, Result};
use async_tungstenite::{tokio::{connect_async, TokioAdapter}, tungstenite::Message};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};

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

/// Messages sent from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        session_id: String,
        peer_id: String,
        join_token: String,
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
    },
    PeerJoined {
        session_id: String,
        peer_id: String,
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
}

impl MultiuserClient {
    pub fn new(server_url: String) -> Self {
        Self {
            server_url,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            message_tx: None,
            event_rx: None,
        }
    }

    /// Get current connection status
    pub async fn status(&self) -> ConnectionStatus {
        self.status.read().await.clone()
    }

    /// Create a new session (generates local credentials)
    /// The server will create the actual session on WebSocket connect
    pub async fn create_session(&self) -> Result<(String, String)> {
        // Generate session credentials locally
        let session_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let join_token = uuid::Uuid::new_v4().to_string()[..16].to_string();

        info!("Generated session credentials: {}", session_id);
        Ok((session_id, join_token))
    }

    /// Connect to a session via WebSocket
    pub async fn connect(
        &mut self,
        session_id: String,
        join_token: String,
    ) -> Result<mpsc::UnboundedReceiver<ServerMessage>> {
        *self.status.write().await = ConnectionStatus::Connecting;

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
        let (event_tx, event_rx) = mpsc::unbounded_channel::<ServerMessage>();

        // Use oneshot channel to get connection result from Tokio runtime
        let (result_tx, result_rx) = oneshot::channel();

        let status_clone = self.status.clone();
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
                    };

                    if let Ok(join_json) = serde_json::to_string(&join_msg) {
                        if let Err(e) = write.send(Message::Text(join_json)).await {
                            error!("Failed to send join message: {}", e);
                            let _ = result_tx.send(Err(anyhow::anyhow!("Failed to send join: {}", e)));
                            return;
                        }
                    }

                    // Signal successful connection
                    let _ = result_tx.send(Ok(()));

                    // Spawn task to handle outgoing messages
                    let status_clone_out = status_clone.clone();
                    tokio::spawn(async move {
                        let mut message_rx = message_rx;
                        while let Some(msg) = message_rx.recv().await {
                            if let Ok(json) = serde_json::to_string(&msg) {
                                if let Err(e) = write.send(Message::Text(json)).await {
                                    error!("Failed to send message: {}", e);
                                    *status_clone_out.write().await = ConnectionStatus::Error(e.to_string());
                                    break;
                                }
                            }
                        }
                    });

                    // Handle incoming messages
                    while let Some(result) = read.next().await {
                        match result {
                            Ok(Message::Text(text)) => {
                                match serde_json::from_str::<ServerMessage>(&text) {
                                    Ok(msg) => {
                                        debug!("Received message: {:?}", msg);

                                        // Update status on successful join
                                        if matches!(msg, ServerMessage::Joined { .. }) {
                                            *status_clone.write().await = ConnectionStatus::Connected;
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
                            Ok(Message::Close(_)) => {
                                info!("WebSocket closed by server");
                                *status_clone.write().await = ConnectionStatus::Disconnected;
                                break;
                            }
                            Ok(Message::Ping(_)) => {
                                // Pings are handled automatically by tungstenite
                            }
                            Ok(_) => {}
                            Err(e) => {
                                error!("WebSocket error: {}", e);
                                *status_clone.write().await = ConnectionStatus::Error(e.to_string());
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to WebSocket: {}", e);
                    *status_clone.write().await = ConnectionStatus::Error(e.to_string());
                    let _ = result_tx.send(Err(anyhow::anyhow!("Connection failed: {}", e)));
                }
            }
        });

        // Wait for connection result from Tokio runtime
        match result_rx.await {
            Ok(Ok(())) => {
                self.message_tx = Some(message_tx);
                Ok(event_rx)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow::anyhow!("Connection task failed")),
        }
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
            let leave_msg = ClientMessage::Leave { session_id, peer_id };
            tx.send(leave_msg)?;
        }

        self.message_tx = None;
        *self.status.write().await = ConnectionStatus::Disconnected;

        Ok(())
    }
}
