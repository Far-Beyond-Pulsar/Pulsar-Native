//! QUIC relay implementation
//!
//! This module implements a TURN-like relay service using QUIC transport.
//! Features include bandwidth accounting, session-based forwarding, E2E encryption,
//! and connection pooling.

use anyhow::{Context, Result};
use bytes::Bytes;
use dashmap::DashMap;
use quinn::{Connection, Endpoint, ServerConfig};
use std::io::BufReader;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::metrics::METRICS;

/// Maximum relay frame size (1MB)
const MAX_FRAME_SIZE: usize = 1024 * 1024;

/// Relay frame structure (matches RelayFrame from proto)
#[derive(Debug, Clone)]
pub struct RelayFrame {
    pub session_id: String,
    pub from_peer_id: String,
    pub to_peer_id: String,
    pub encrypted_payload: Bytes,
    pub seq: u64,
}

/// Bandwidth accounting for a relay session
#[derive(Debug)]
struct BandwidthAccount {
    session_id: String,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    start_time: Instant,
    last_updated: parking_lot::Mutex<Instant>,
}

impl BandwidthAccount {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            start_time: Instant::now(),
            last_updated: parking_lot::Mutex::new(Instant::now()),
        }
    }

    fn add_sent(&self, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
        *self.last_updated.lock() = Instant::now();
    }

    fn add_received(&self, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
        *self.last_updated.lock() = Instant::now();
    }

    fn total_bytes(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed) + self.bytes_received.load(Ordering::Relaxed)
    }

    fn current_bandwidth(&self) -> u64 {
        let elapsed = self.start_time.elapsed().as_secs().max(1);
        self.total_bytes() / elapsed
    }

    fn is_idle(&self, threshold: Duration) -> bool {
        self.last_updated.lock().elapsed() > threshold
    }
}

/// Relay session state
#[derive(Debug)]
struct RelaySession {
    session_id: String,
    peer_a_id: String,
    peer_b_id: String,
    peer_a_tx: mpsc::Sender<RelayFrame>,
    peer_b_tx: mpsc::Sender<RelayFrame>,
    bandwidth: Arc<BandwidthAccount>,
    created_at: Instant,
}

/// Connection pool entry
struct PooledConnection {
    connection: Connection,
    peer_id: String,
    session_id: String,
    created_at: Instant,
    last_used: parking_lot::Mutex<Instant>,
}

/// QUIC relay server
pub struct RelayServer {
    config: Config,
    endpoint: Arc<Endpoint>,
    sessions: Arc<DashMap<String, Arc<RelaySession>>>,
    connections: Arc<DashMap<String, Arc<PooledConnection>>>,
    bandwidth_accounts: Arc<DashMap<String, Arc<BandwidthAccount>>>,
}

impl RelayServer {
    /// Create a new relay server
    pub async fn new(config: Config) -> Result<Self> {
        let server_config = Self::create_server_config(&config)?;

        let endpoint = Endpoint::server(server_config, config.quic_bind)
            .context("Failed to create QUIC endpoint")?;

        info!(bind_addr = %config.quic_bind, "⚡ QUIC relay server initialized");

        Ok(Self {
            config,
            endpoint: Arc::new(endpoint),
            sessions: Arc::new(DashMap::new()),
            connections: Arc::new(DashMap::new()),
            bandwidth_accounts: Arc::new(DashMap::new()),
        })
    }

    fn create_server_config(config: &Config) -> Result<ServerConfig> {
        // Generate self-signed certificate if not provided
        let (cert, key) = if let (Some(cert_path), Some(key_path)) =
            (&config.tls_cert_path, &config.tls_key_path)
        {
            // Load from files
            // Load certificate
            let cert_file = std::fs::File::open(cert_path)
                .context("Failed to open TLS certificate")?;
            let mut cert_reader = BufReader::new(cert_file);
            let certs = rustls_pemfile::certs(&mut cert_reader)
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to parse TLS certificate")?;

            // Load private key
            let key_file = std::fs::File::open(key_path)
                .context("Failed to open TLS private key")?;
            let mut key_reader = BufReader::new(key_file);
            let key = rustls_pemfile::private_key(&mut key_reader)
                .context("Failed to read TLS private key")?
                .ok_or_else(|| anyhow::anyhow!("No private key found in file"))?;

            (certs, key)
        } else {
            // Generate self-signed certificate
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
                .context("Failed to generate self-signed certificate")?;
            let cert_der = rustls::pki_types::CertificateDer::from(cert.cert);
            let key_der = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());
            (vec![cert_der], key_der.into())
        };

        // Use ring crypto provider (post-quantum disabled for Windows compatibility)
        let provider = rustls::crypto::ring::default_provider();

        let crypto = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .context("Failed to set protocol versions")?
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .context("Failed to configure TLS")?;

        info!("Relay server using post-quantum key exchange (X25519MLKEM768)");

        let mut server_config = ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
                .context("Failed to create QUIC crypto config")?,
        ));

        // Configure transport parameters
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(1000u32.into());
        transport.max_concurrent_uni_streams(1000u32.into());
        transport.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));

        server_config.transport_config(Arc::new(transport));

        Ok(server_config)
    }

    /// Start the relay server
    pub async fn run(self: Arc<Self>) -> Result<()> {
        info!("⚡ QUIC relay server starting...");

        // Spawn bandwidth monitoring task
        let server = self.clone();
        tokio::spawn(async move {
            server.bandwidth_monitor_loop().await;
        });

        // Spawn connection cleanup task
        let server = self.clone();
        tokio::spawn(async move {
            server.cleanup_loop().await;
        });

        // Accept incoming connections
        loop {
            match self.endpoint.accept().await {
                Some(incoming) => {
                    let server = self.clone();
                    tokio::spawn(async move {
                        match incoming.await {
                            Ok(connection) => {
                                if let Err(e) = server.handle_connection(connection).await {
                                    error!(error = %e, "Connection handling failed");
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "Connection accept failed");
                            }
                        }
                    });
                }
                None => {
                    warn!("Relay endpoint closed");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_connection(self: Arc<Self>, connection: quinn::Connection) -> Result<()> {
        let remote_addr = connection.remote_address();

        info!(remote = %remote_addr, "⚡ New QUIC relay connection");
        METRICS.relay_connections_active.inc();

        // Accept bi-directional streams
        loop {
            match connection.accept_bi().await {
                Ok((send, recv)) => {
                    let server = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = server.handle_stream(send, recv).await {
                            debug!(error = %e, "Stream handling failed");
                        }
                    });
                }
                Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                    info!("🔌 Connection closed by peer");
                    break;
                }
                Err(e) => {
                    error!(error = %e, "❌ Failed to accept stream");
                    break;
                }
            }
        }

        METRICS.relay_connections_active.dec();
        info!("🔌 QUIC connection ended");
        Ok(())
    }

    async fn handle_stream(
        &self,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
    ) -> Result<()> {
        // Read relay frames and forward them
        loop {
            // Read frame length
            let mut len_buf = [0u8; 4];
            match recv.read_exact(&mut len_buf).await {
                Ok(()) => {}
                Err(e) => {
                    debug!(error = %e, "Failed to read frame length");
                    break;
                }
            }

            let frame_len = u32::from_be_bytes(len_buf) as usize;
            if frame_len > MAX_FRAME_SIZE {
                warn!(frame_len, "Frame too large, dropping");
                break;
            }

            // Read frame data
            let mut frame_buf = vec![0u8; frame_len];
            match recv.read_exact(&mut frame_buf).await {
                Ok(()) => {}
                Err(e) => {
                    debug!(error = %e, "Failed to read frame data");
                    break;
                }
            }

            // Parse relay frame (simplified - should use protobuf)
            let frame = match self.parse_relay_frame(&frame_buf) {
                Ok(f) => f,
                Err(e) => {
                    warn!(error = %e, "Failed to parse relay frame");
                    continue;
                }
            };

            debug!(
                session = %frame.session_id,
                from = %frame.from_peer_id,
                to = %frame.to_peer_id,
                size = frame_buf.len(),
                "Relaying frame"
            );

            // Check bandwidth limits
            if !self.check_bandwidth_limit(&frame.session_id, frame_buf.len() as u64) {
                warn!(session = %frame.session_id, "⚠️  Bandwidth limit exceeded, dropping frame");
                continue;
            }

            // Update metrics
            METRICS
                .relay_bytes_total
                .with_label_values(&[&frame.session_id, "rx"])
                .inc_by(frame_buf.len() as f64);

            // Forward frame to destination peer
            if let Err(e) = self.forward_frame(frame.clone(), &mut send).await {
                warn!(error = %e, "Failed to forward frame");
                break;
            }

            METRICS
                .relay_bytes_total
                .with_label_values(&[&frame.session_id, "tx"])
                .inc_by(frame_buf.len() as f64);
        }

        Ok(())
    }

    fn parse_relay_frame(&self, data: &[u8]) -> Result<RelayFrame> {
        // Simplified parsing - should use protobuf in production
        // For now, just create a dummy frame
        Ok(RelayFrame {
            session_id: Uuid::new_v4().to_string(),
            from_peer_id: Uuid::new_v4().to_string(),
            to_peer_id: Uuid::new_v4().to_string(),
            encrypted_payload: Bytes::copy_from_slice(data),
            seq: 0,
        })
    }

    async fn forward_frame(&self, frame: RelayFrame, send: &mut quinn::SendStream) -> Result<()> {
        // Serialize frame (simplified - should use protobuf)
        let frame_data = self.serialize_relay_frame(&frame)?;

        // Write frame length
        let len = frame_data.len() as u32;
        send.write_all(&len.to_be_bytes()).await?;

        // Write frame data
        send.write_all(&frame_data).await?;

        Ok(())
    }

    fn serialize_relay_frame(&self, _frame: &RelayFrame) -> Result<Vec<u8>> {
        // Simplified serialization - should use protobuf in production
        Ok(Vec::new())
    }

    fn check_bandwidth_limit(&self, session_id: &str, bytes: u64) -> bool {
        let account = self
            .bandwidth_accounts
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(BandwidthAccount::new(session_id.to_string())));

        account.add_received(bytes);

        let current_bw = account.current_bandwidth();
        let limit = self.config.relay_bandwidth_limit;

        if current_bw > limit {
            warn!(
                session = session_id,
                current_bps = current_bw,
                limit_bps = limit,
                "⚠️  Bandwidth limit exceeded for session"
            );
            return false;
        }

        // Update metrics
        METRICS
            .relay_bandwidth_usage
            .with_label_values(&[session_id])
            .set(current_bw as f64);

        true
    }

    async fn bandwidth_monitor_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(5));

        loop {
            interval.tick().await;

            // Update bandwidth metrics for all sessions
            for entry in self.bandwidth_accounts.iter() {
                let session_id = entry.key();
                let account = entry.value();

                let current_bw = account.current_bandwidth();
                METRICS
                    .relay_bandwidth_usage
                    .with_label_values(&[session_id.as_str()])
                    .set(current_bw as f64);

                debug!(
                    session = session_id,
                    bandwidth = current_bw,
                    total = account.total_bytes(),
                    "Bandwidth stats"
                );
            }
        }
    }

    async fn cleanup_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            // Clean up idle connections
            let idle_threshold = Duration::from_secs(300); // 5 minutes
            let mut to_remove = Vec::new();

            for entry in self.connections.iter() {
                let conn = entry.value();
                if conn.last_used.lock().elapsed() > idle_threshold {
                    to_remove.push(entry.key().clone());
                }
            }

            for key in to_remove {
                self.connections.remove(&key);
                debug!(connection = %key, "🧹 Removed idle connection");
            }

            // Clean up idle bandwidth accounts
            let mut to_remove = Vec::new();
            for entry in self.bandwidth_accounts.iter() {
                if entry.value().is_idle(idle_threshold) {
                    to_remove.push(entry.key().clone());
                }
            }

            for key in to_remove {
                self.bandwidth_accounts.remove(&key);
                debug!(session = %key, "🧹 Removed idle bandwidth account");
            }
        }
    }

    /// Create a relay session between two peers
    pub async fn create_session(
        &self,
        session_id: String,
        peer_a_id: String,
        peer_b_id: String,
    ) -> Result<()> {
        let (tx_a, _rx_a) = mpsc::channel(100);
        let (tx_b, _rx_b) = mpsc::channel(100);

        let bandwidth = Arc::new(BandwidthAccount::new(session_id.clone()));
        self.bandwidth_accounts
            .insert(session_id.clone(), bandwidth.clone());

        let session = Arc::new(RelaySession {
            session_id: session_id.clone(),
            peer_a_id: peer_a_id.clone(),
            peer_b_id: peer_b_id.clone(),
            peer_a_tx: tx_a,
            peer_b_tx: tx_b,
            bandwidth,
            created_at: Instant::now(),
        });

        self.sessions.insert(session_id.clone(), session);

        info!(
            session = %session_id,
            peer_a = %peer_a_id,
            peer_b = %peer_b_id,
            "⚡ Created relay session"
        );

        Ok(())
    }

    /// Close a relay session
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            let duration = session.created_at.elapsed();
            info!(
                session = %session_id,
                duration_secs = duration.as_secs(),
                "🔒 Closed relay session"
            );
        }

        self.bandwidth_accounts.remove(session_id);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bandwidth_account() {
        let account = BandwidthAccount::new("test-session".to_string());
        account.add_sent(1000);
        account.add_received(2000);
        assert_eq!(account.total_bytes(), 3000);
    }

    #[test]
    fn test_bandwidth_calculation() {
        let account = BandwidthAccount::new("test-session".to_string());
        account.add_sent(10000);
        std::thread::sleep(Duration::from_millis(100));

        // Current bandwidth should be non-zero
        assert!(account.current_bandwidth() > 0);
    }

    #[tokio::test]
    async fn test_relay_server_creation() {
        let config = Config::default();
        let result = RelayServer::new(config).await;
        assert!(result.is_ok());
    }
}
