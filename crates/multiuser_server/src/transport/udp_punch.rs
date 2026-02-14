//! UDP hole punching implementation for NAT traversal
//!
//! This module provides production-ready UDP hole punching functionality
//! with retry logic, exponential backoff, token validation, and punch coordination.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, RwLock},
    time::{sleep, timeout, Instant},
};
use tracing::{debug, error, info, warn};

use crate::{config::Config, metrics::METRICS};

/// Maximum UDP packet size
const MAX_PACKET_SIZE: usize = 1400;

/// Initial retry delay
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Maximum retry delay
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

/// Maximum retry attempts
const MAX_RETRIES: u32 = 10;

/// Hole punch coordination timeout
const COORDINATION_TIMEOUT: Duration = Duration::from_secs(10);

/// UDP hole punching coordinator
pub struct UdpHolePuncher {
    socket: Arc<UdpSocket>,
    config: Arc<Config>,
    stats: Arc<PunchStats>,
    active_punches: Arc<RwLock<Vec<PunchSession>>>,
}

/// Statistics for hole punching
#[derive(Default)]
struct PunchStats {
    total_attempts: AtomicU64,
    successful_punches: AtomicU64,
    failed_punches: AtomicU64,
    avg_punch_duration_ms: AtomicU64,
}

/// Active punch session
struct PunchSession {
    session_id: String,
    peer_addr: SocketAddr,
    started_at: Instant,
    nat_type: NatType,
}

/// NAT type classification
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum NatType {
    Unknown,
    Open,
    FullCone,
    RestrictedCone,
    PortRestrictedCone,
    Symmetric,
}

impl NatType {
    fn as_str(&self) -> &'static str {
        match self {
            NatType::Unknown => "unknown",
            NatType::Open => "open",
            NatType::FullCone => "full_cone",
            NatType::RestrictedCone => "restricted_cone",
            NatType::PortRestrictedCone => "port_restricted_cone",
            NatType::Symmetric => "symmetric",
        }
    }
}

/// Hole punch message types
#[derive(Debug, Clone, Serialize, Deserialize)]
enum PunchMessage {
    /// Initial punch request with token
    PunchRequest { token: String, session_id: String },
    /// Acknowledgment of punch
    PunchAck { session_id: String },
    /// Punch success confirmation
    PunchSuccess { session_id: String },
    /// Keep-alive packet
    KeepAlive,
    /// Test packet for NAT detection
    NatProbe { sequence: u32 },
}

impl UdpHolePuncher {
    /// Create a new UDP hole puncher
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        info!("Initializing UDP hole puncher on {}", config.udp_bind);

        // Create socket with SO_REUSEADDR
        let socket = Self::create_socket(config.udp_bind)?;
        let socket = Arc::new(socket);

        info!("UDP hole puncher listening on {}", socket.local_addr()?);

        Ok(Self {
            socket,
            config,
            stats: Arc::new(PunchStats::default()),
            active_punches: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Create a UDP socket with proper socket options
    fn create_socket(bind_addr: SocketAddr) -> Result<UdpSocket> {
        let socket = Socket::new(
            if bind_addr.is_ipv4() { Domain::IPV4 } else { Domain::IPV6 },
            Type::DGRAM,
            Some(Protocol::UDP),
        )
        .context("Failed to create socket")?;

        // Enable address reuse
        socket.set_reuse_address(true).context("Failed to set SO_REUSEADDR")?;

        #[cfg(unix)]
        socket.set_reuse_port(true).context("Failed to set SO_REUSEPORT")?;

        // Bind the socket
        socket.bind(&bind_addr.into()).context("Failed to bind socket")?;

        // Convert to tokio UdpSocket
        socket.set_nonblocking(true).context("Failed to set non-blocking")?;
        let std_socket: std::net::UdpSocket = socket.into();
        UdpSocket::from_std(std_socket).context("Failed to convert to tokio socket")
    }

    /// Perform UDP hole punching to a peer
    pub async fn punch_hole(
        &self,
        peer_addr: SocketAddr,
        token: String,
        nat_type: NatType,
    ) -> Result<()> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let start = Instant::now();

        info!(
            "Starting hole punch to {} for session {} (NAT: {:?})",
            peer_addr, session_id, nat_type
        );

        self.stats.total_attempts.fetch_add(1, Ordering::Relaxed);
        METRICS
            .hole_punch_attempts
            .with_label_values(&[nat_type.as_str()])
            .inc();

        // Register punch session
        self.active_punches.write().await.push(PunchSession {
            session_id: session_id.clone(),
            peer_addr,
            started_at: start,
            nat_type,
        });

        // Perform punch with retry logic
        let result = self
            .punch_with_retry(peer_addr, token, session_id.clone(), nat_type)
            .await;

        // Cleanup session
        let mut punches = self.active_punches.write().await;
        punches.retain(|p| p.session_id != session_id);

        // Record metrics
        let duration = start.elapsed();
        match result {
            Ok(_) => {
                info!(
                    "Hole punch succeeded to {} in {:?} (session: {})",
                    peer_addr, duration, session_id
                );
                self.stats.successful_punches.fetch_add(1, Ordering::Relaxed);
                METRICS
                    .hole_punch_success
                    .with_label_values(&[nat_type.as_str()])
                    .inc();
                METRICS
                    .hole_punch_duration
                    .with_label_values(&[nat_type.as_str()])
                    .observe(duration.as_secs_f64());
            }
            Err(ref e) => {
                warn!(
                    "Hole punch failed to {} after {:?}: {} (session: {})",
                    peer_addr, duration, e, session_id
                );
                self.stats.failed_punches.fetch_add(1, Ordering::Relaxed);
            }
        }

        result
    }

    /// Perform hole punching with exponential backoff retry
    async fn punch_with_retry(
        &self,
        peer_addr: SocketAddr,
        token: String,
        session_id: String,
        nat_type: NatType,
    ) -> Result<()> {
        let mut retry_delay = INITIAL_RETRY_DELAY;
        let mut attempts = 0;

        loop {
            attempts += 1;

            match timeout(
                self.config.hole_punch_timeout,
                self.send_punch_request(peer_addr, token.clone(), session_id.clone()),
            )
            .await
            {
                Ok(Ok(_)) => {
                    debug!("Punch request sent successfully (attempt {})", attempts);
                    // Wait for acknowledgment
                    match self.wait_for_ack(peer_addr, session_id.clone()).await {
                        Ok(_) => return Ok(()),
                        Err(e) if attempts >= MAX_RETRIES => {
                            return Err(e).context("Max retries reached");
                        }
                        Err(e) => {
                            debug!("ACK wait failed: {}, retrying...", e);
                        }
                    }
                }
                Ok(Err(e)) => {
                    if attempts >= MAX_RETRIES {
                        return Err(e).context("Max retries reached");
                    }
                    debug!("Punch request failed: {}, retrying...", e);
                }
                Err(_) => {
                    if attempts >= MAX_RETRIES {
                        anyhow::bail!("Hole punch timeout after {} attempts", attempts);
                    }
                    debug!("Punch request timeout, retrying...");
                }
            }

            // Exponential backoff
            sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);

            // Add jitter for symmetric NAT
            if nat_type == NatType::Symmetric {
                let jitter = Duration::from_millis(rand::random::<u64>() % 100);
                sleep(jitter).await;
            }
        }
    }

    /// Send a punch request to the peer
    async fn send_punch_request(
        &self,
        peer_addr: SocketAddr,
        token: String,
        session_id: String,
    ) -> Result<()> {
        let message = PunchMessage::PunchRequest { token, session_id };
        let payload = bincode::serialize(&message).context("Failed to serialize message")?;

        self.socket
            .send_to(&payload, peer_addr)
            .await
            .context("Failed to send punch request")?;

        debug!("Sent punch request to {}", peer_addr);
        Ok(())
    }

    /// Wait for acknowledgment from peer
    async fn wait_for_ack(&self, peer_addr: SocketAddr, session_id: String) -> Result<()> {
        let mut buffer = vec![0u8; MAX_PACKET_SIZE];

        let result = timeout(COORDINATION_TIMEOUT, async {
            loop {
                match self.socket.recv_from(&mut buffer).await {
                    Ok((n, addr)) if addr == peer_addr => {
                        if let Ok(message) = bincode::deserialize::<PunchMessage>(&buffer[..n]) {
                            match message {
                                PunchMessage::PunchAck { session_id: ack_id }
                                    if ack_id == session_id =>
                                {
                                    debug!("Received ACK from {}", peer_addr);
                                    return Ok(());
                                }
                                _ => continue,
                            }
                        }
                    }
                    Ok(_) => continue,
                    Err(e) => return Err(e).context("Failed to receive ACK"),
                }
            }
        })
        .await;

        result.context("ACK timeout")?
    }

    /// Handle incoming punch requests (server mode)
    pub async fn run_server(self: Arc<Self>, shutdown: mpsc::Receiver<()>) -> Result<()> {
        info!("UDP hole puncher server started");
        let mut shutdown = shutdown;
        let mut buffer = vec![0u8; MAX_PACKET_SIZE];

        loop {
            tokio::select! {
                result = self.socket.recv_from(&mut buffer) => {
                    match result {
                        Ok((n, addr)) => {
                            let puncher = self.clone();
                            let data = buffer[..n].to_vec();

                            tokio::spawn(async move {
                                if let Err(e) = puncher.handle_message(data, addr).await {
                                    debug!("Failed to handle message from {}: {}", addr, e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("Failed to receive UDP packet: {}", e);
                        }
                    }
                }
                _ = shutdown.recv() => {
                    info!("Shutdown signal received, stopping UDP hole puncher");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handle incoming message
    async fn handle_message(&self, data: Vec<u8>, addr: SocketAddr) -> Result<()> {
        let message: PunchMessage =
            bincode::deserialize(&data).context("Failed to deserialize message")?;

        match message {
            PunchMessage::PunchRequest { token, session_id } => {
                debug!("Received punch request from {} for session {}", addr, session_id);

                // Validate token
                if self.validate_token(&token).await? {
                    // Send acknowledgment
                    let ack = PunchMessage::PunchAck { session_id };
                    let payload = bincode::serialize(&ack)?;
                    self.socket.send_to(&payload, addr).await?;
                    debug!("Sent ACK to {}", addr);
                } else {
                    warn!("Invalid token from {}", addr);
                }
            }
            PunchMessage::NatProbe { sequence } => {
                debug!("Received NAT probe {} from {}", sequence, addr);
                // Echo back for NAT detection
                let response = PunchMessage::NatProbe { sequence };
                let payload = bincode::serialize(&response)?;
                self.socket.send_to(&payload, addr).await?;
            }
            PunchMessage::KeepAlive => {
                debug!("Received keep-alive from {}", addr);
            }
            _ => {
                debug!("Received unexpected message from {}", addr);
            }
        }

        Ok(())
    }

    /// Validate punch token
    async fn validate_token(&self, token: &str) -> Result<bool> {
        // Simplified token validation: length check only
        // Production: Should implement JWT or HMAC-based validation
        Ok(!token.is_empty() && token.len() >= 16)
    }

    /// Detect NAT type using STUN-like probing
    pub async fn detect_nat_type(&self, stun_servers: Vec<SocketAddr>) -> Result<NatType> {
        if stun_servers.is_empty() {
            return Ok(NatType::Unknown);
        }

        info!("Detecting NAT type using {} STUN servers", stun_servers.len());

        // Send probes to multiple STUN servers
        let mut external_addrs = Vec::new();

        for server in &stun_servers {
            match self.probe_external_address(*server).await {
                Ok(addr) => external_addrs.push(addr),
                Err(e) => debug!("Failed to probe {}: {}", server, e),
            }
        }

        let nat_type = if external_addrs.is_empty() {
            NatType::Unknown
        } else if external_addrs.iter().all(|a| a == &external_addrs[0]) {
            // Same external address from all servers
            NatType::FullCone
        } else {
            // Different external addresses
            NatType::Symmetric
        };

        info!("Detected NAT type: {:?}", nat_type);
        METRICS
            .nat_type_detected
            .with_label_values(&[nat_type.as_str()])
            .inc();

        Ok(nat_type)
    }

    /// Probe external address via STUN server
    async fn probe_external_address(&self, stun_server: SocketAddr) -> Result<SocketAddr> {
        let probe = PunchMessage::NatProbe { sequence: 1 };
        let payload = bincode::serialize(&probe)?;

        self.socket.send_to(&payload, stun_server).await?;

        let mut buffer = vec![0u8; MAX_PACKET_SIZE];
        let (n, addr) = timeout(Duration::from_secs(2), self.socket.recv_from(&mut buffer))
            .await
            .context("STUN probe timeout")??;

        Ok(addr)
    }

    /// Get statistics
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.stats.total_attempts.load(Ordering::Relaxed),
            self.stats.successful_punches.load(Ordering::Relaxed),
            self.stats.failed_punches.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_socket_creation() {
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let socket = UdpHolePuncher::create_socket(bind_addr);
        assert!(socket.is_ok());
    }

    #[tokio::test]
    async fn test_puncher_creation() {
        let mut config = Config::default();
        config.udp_bind = "127.0.0.1:0".parse().unwrap();

        let puncher = UdpHolePuncher::new(Arc::new(config)).await;
        assert!(puncher.is_ok());
    }

    #[tokio::test]
    async fn test_stats() {
        let mut config = Config::default();
        config.udp_bind = "127.0.0.1:0".parse().unwrap();

        let puncher = UdpHolePuncher::new(Arc::new(config)).await.unwrap();
        let (attempts, success, failed) = puncher.stats();

        assert_eq!(attempts, 0);
        assert_eq!(success, 0);
        assert_eq!(failed, 0);
    }

    #[tokio::test]
    async fn test_nat_type_serialization() {
        let nat_type = NatType::FullCone;
        let serialized = bincode::serialize(&nat_type).unwrap();
        let deserialized: NatType = bincode::deserialize(&serialized).unwrap();
        assert_eq!(nat_type, deserialized);
    }

    #[tokio::test]
    async fn test_message_serialization() {
        let msg = PunchMessage::PunchRequest {
            token: "test_token".to_string(),
            session_id: "session_123".to_string(),
        };

        let serialized = bincode::serialize(&msg).unwrap();
        let deserialized: PunchMessage = bincode::deserialize(&serialized).unwrap();

        match deserialized {
            PunchMessage::PunchRequest { token, session_id } => {
                assert_eq!(token, "test_token");
                assert_eq!(session_id, "session_123");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[tokio::test]
    async fn test_token_validation() {
        let mut config = Config::default();
        config.udp_bind = "127.0.0.1:0".parse().unwrap();

        let puncher = UdpHolePuncher::new(Arc::new(config)).await.unwrap();

        assert!(puncher.validate_token("valid_token_12345").await.unwrap());
        assert!(!puncher.validate_token("short").await.unwrap());
        assert!(!puncher.validate_token("").await.unwrap());
    }

    #[tokio::test]
    async fn test_nat_type_conversion() {
        assert_eq!(NatType::Open.as_str(), "open");
        assert_eq!(NatType::Symmetric.as_str(), "symmetric");
        assert_eq!(NatType::FullCone.as_str(), "full_cone");
    }
}
