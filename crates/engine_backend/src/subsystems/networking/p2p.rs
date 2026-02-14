//! Peer-to-peer connection management with hole punching
//!
//! ## Connection Strategy (in order of preference):
//!
//! 1. **Direct P2P (Hole Punch)**: STUN/ICE NAT traversal → TCP → Git native protocol
//!    - Fastest, lowest latency
//!    - Uses git's pack protocol directly
//!    - No server bandwidth usage
//!
//! 2. **Binary Proxy**: WebSocket → Raw git bytes tunneled through server
//!    - Fast, low overhead
//!    - Server proxies bytes without parsing
//!    - Works through any firewall
//!
//! 3. **JSON Fallback**: WebSocket → Serialized git objects as JSON
//!    - Slowest, highest overhead
//!    - Last resort for problematic networks

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::RwLock;

/// Default public STUN servers for NAT traversal
const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

/// STUN server configuration
#[derive(Debug, Clone)]
pub struct StunConfig {
    pub servers: Vec<String>,
    pub timeout_ms: u64,
}

impl Default for StunConfig {
    fn default() -> Self {
        Self {
            servers: DEFAULT_STUN_SERVERS.iter().map(|s| s.to_string()).collect(),
            timeout_ms: 5000,
        }
    }
}

/// Connection mode for peer-to-peer sync
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionMode {
    /// Direct P2P connection via hole punching
    DirectP2P,
    /// Binary proxy through WebSocket server
    BinaryProxy,
    /// JSON-based fallback
    JsonFallback,
}

/// P2P connection state
#[derive(Debug, Clone)]
pub struct P2PConnection {
    pub mode: ConnectionMode,
    pub peer_address: Option<SocketAddr>,
    pub latency_ms: Option<u64>,
}

/// ICE candidate for NAT traversal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceCandidate {
    pub candidate: String,
    pub sdp_mid: String,
    pub sdp_m_line_index: u16,
}

/// Connection negotiation messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum P2PMessage {
    /// Request to establish P2P connection
    ConnectionRequest {
        session_id: String,
        peer_id: String,
        public_ip: String,
        public_port: u16,
    },
    /// Response with connection details
    ConnectionResponse {
        session_id: String,
        peer_id: String,
        public_ip: String,
        public_port: u16,
        ice_candidates: Vec<IceCandidate>,
    },
    /// Hole punching keepalive
    Keepalive {
        peer_id: String,
    },
    /// Request binary proxy mode
    RequestBinaryProxy {
        session_id: String,
        peer_id: String,
    },
    /// Binary data chunk (proxied through server)
    BinaryChunk {
        session_id: String,
        peer_id: String,
        data: Vec<u8>,
        sequence: u64,
    },
}

/// P2P connection manager
pub struct P2PManager {
    mode: Arc<RwLock<ConnectionMode>>,
    connection: Arc<RwLock<Option<TcpStream>>>,
    stun_config: StunConfig,
}

impl P2PManager {
    pub fn new() -> Self {
        Self {
            mode: Arc::new(RwLock::new(ConnectionMode::DirectP2P)),
            connection: Arc::new(RwLock::new(None)),
            stun_config: StunConfig::default(),
        }
    }

    /// Create manager with custom STUN configuration
    pub fn with_stun_config(stun_config: StunConfig) -> Self {
        Self {
            mode: Arc::new(RwLock::new(ConnectionMode::DirectP2P)),
            connection: Arc::new(RwLock::new(None)),
            stun_config,
        }
    }

    /// Attempt to establish P2P connection with fallback chain
    pub async fn connect(&self, peer_address: &str) -> Result<ConnectionMode, String> {
        // Try 1: Direct P2P with hole punching
        tracing::debug!("Attempting direct P2P connection to {}", peer_address);
        if let Ok(stream) = self.try_direct_p2p(peer_address).await {
            *self.connection.write().await = Some(stream);
            *self.mode.write().await = ConnectionMode::DirectP2P;
            tracing::debug!("✓ Direct P2P connection established");
            return Ok(ConnectionMode::DirectP2P);
        }

        // Try 2: Binary proxy through server
        tracing::debug!("Direct P2P failed, trying binary proxy");
        if self.try_binary_proxy().await.is_ok() {
            *self.mode.write().await = ConnectionMode::BinaryProxy;
            tracing::debug!("✓ Binary proxy mode established");
            return Ok(ConnectionMode::BinaryProxy);
        }

        // Fallback 3: JSON messages
        tracing::warn!("Binary proxy failed, falling back to JSON mode");
        *self.mode.write().await = ConnectionMode::JsonFallback;
        Ok(ConnectionMode::JsonFallback)
    }

    /// Try to establish direct TCP connection with NAT traversal
    async fn try_direct_p2p(&self, peer_address: &str) -> Result<TcpStream, std::io::Error> {
        // Step 1: Query STUN servers to determine public IP and NAT type
        tracing::info!("Attempting STUN query to servers: {:?}", self.stun_config.servers);
        
        // TODO: Implement actual STUN binding request
        // This would use a STUN client library to:
        // - Send binding requests to STUN servers
        // - Parse responses to get public IP and port
        // - Determine NAT type (Full Cone, Restricted, Port Restricted, Symmetric)
        
        // Step 2: Exchange ICE candidates with peer through signaling server
        // TODO: Send our candidate (public IP:port) to peer via WebSocket signaling
        // TODO: Receive peer's candidate from signaling server
        
        // Step 3: Attempt simultaneous TCP connection (hole punching)
        // For symmetric NATs, both sides connect at the same time
        // This creates temporary port mapping that allows connection
        
        tracing::debug!("Attempting direct connection to peer: {}", peer_address);
        
        // Currently: Simple direct connection attempt
        // Will be replaced with full ICE connection establishment
        tokio::time::timeout(
            std::time::Duration::from_millis(self.stun_config.timeout_ms),
            TcpStream::connect(peer_address),
        )
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Connection timeout"))?
    }

    /// Request binary proxy mode from server
    async fn try_binary_proxy(&self) -> Result<(), String> {
        // Server will need to implement binary tunneling
        // For now, return error to force JSON fallback
        Err("Binary proxy not yet implemented".to_string())
    }

    /// Get current connection mode
    pub async fn get_mode(&self) -> ConnectionMode {
        *self.mode.read().await
    }

    /// Send git data through active connection
    pub async fn send_git_data(&self, data: &[u8]) -> Result<(), String> {
        match *self.mode.read().await {
            ConnectionMode::DirectP2P => {
                // Write directly to TCP stream
                if let Some(stream) = self.connection.write().await.as_mut() {
                    use tokio::io::AsyncWriteExt;
                    stream.write_all(data).await.map_err(|e| e.to_string())?;
                    Ok(())
                } else {
                    Err("No P2P connection".to_string())
                }
            }
            ConnectionMode::BinaryProxy => {
                // Send through WebSocket as binary message
                // TODO: Implement via multiuser_client
                Err("Binary proxy not implemented".to_string())
            }
            ConnectionMode::JsonFallback => {
                // Use existing JSON message system
                Err("Use JSON message fallback".to_string())
            }
        }
    }

    /// Receive git data from active connection
    pub async fn receive_git_data(&self, buffer: &mut [u8]) -> Result<usize, String> {
        match *self.mode.read().await {
            ConnectionMode::DirectP2P => {
                if let Some(stream) = self.connection.write().await.as_mut() {
                    use tokio::io::AsyncReadExt;
                    stream.read(buffer).await.map_err(|e| e.to_string())
                } else {
                    Err("No P2P connection".to_string())
                }
            }
            ConnectionMode::BinaryProxy => {
                // Receive from WebSocket binary message
                Err("Binary proxy not implemented".to_string())
            }
            ConnectionMode::JsonFallback => {
                Err("Use JSON message fallback".to_string())
            }
        }
    }
}

/// Helper to run git fetch/push over a custom transport
pub async fn run_git_over_connection(
    repo_path: &std::path::Path,
    p2p: &P2PManager,
    remote_url: &str,
) -> Result<(), String> {
    match p2p.get_mode().await {
        ConnectionMode::DirectP2P | ConnectionMode::BinaryProxy => {
            // Use git's native protocol over our connection
            // This requires implementing a custom git transport
            tracing::debug!("Running git protocol over {:?}", p2p.get_mode().await);

            // TODO: Implement custom git transport that uses P2PManager
            // For now, use libgit2's callbacks to intercept network I/O

            Err("Custom git transport not yet implemented".to_string())
        }
        ConnectionMode::JsonFallback => {
            // Use existing JSON serialization approach
            Ok(())
        }
    }
}
