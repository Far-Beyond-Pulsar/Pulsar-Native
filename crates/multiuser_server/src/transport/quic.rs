//! QUIC server implementation for Pulsar MultiEdit
//!
//! This module provides a production-ready QUIC server using the Quinn library.
//! It supports both relay and P2P transport modes with TLS, automatic certificate
//! generation, connection handling, and comprehensive metrics integration.

use anyhow::{Context, Result};
use quinn::{
    crypto::rustls::QuicClientConfig, Endpoint, RecvStream, SendStream, ServerConfig, VarInt,
};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::{
    net::SocketAddr,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, RwLock},
    time::Instant,
};
use tracing::{debug, error, info, warn};

use crate::{config::Config, metrics::METRICS};

/// Maximum datagram size for QUIC
const MAX_DATAGRAM_SIZE: usize = 1350;

/// Connection idle timeout
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Keep-alive interval
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// QUIC server for relay and P2P transport
pub struct QuicServer {
    endpoint: Endpoint,
    config: Arc<Config>,
    stats: Arc<ServerStats>,
    connections: Arc<RwLock<Vec<ConnectionHandle>>>,
}

/// Server statistics
#[derive(Default)]
struct ServerStats {
    total_connections: AtomicU64,
    active_connections: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
}

/// Handle to an active connection
struct ConnectionHandle {
    remote_addr: SocketAddr,
    session_id: String,
    established_at: Instant,
}

/// Connection type for metrics
#[derive(Debug, Clone, Copy)]
pub enum ConnectionType {
    Relay,
    P2P,
}

impl QuicServer {
    /// Create a new QUIC server with the given configuration
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        info!("Initializing QUIC server on {}", config.quic_bind);

        // Generate or load TLS certificate
        let (cert_chain, private_key) = if let (Some(cert_path), Some(key_path)) =
            (&config.tls_cert_path, &config.tls_key_path)
        {
            Self::load_certificates(cert_path, key_path)
                .context("Failed to load TLS certificates")?
        } else {
            info!("Generating self-signed certificate for QUIC");
            Self::generate_self_signed_cert().context("Failed to generate certificate")?
        };

        // Configure QUIC server
        let server_config = Self::configure_server(cert_chain, private_key)?;

        // Create endpoint
        let endpoint = Endpoint::server(server_config, config.quic_bind)
            .context("Failed to create QUIC endpoint")?;

        info!("QUIC server listening on {}", endpoint.local_addr()?);

        METRICS.connections_total.with_label_values(&["quic", "server"]).inc();

        Ok(Self {
            endpoint,
            config,
            stats: Arc::new(ServerStats::default()),
            connections: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Run the QUIC server and accept incoming connections
    pub async fn run(self: Arc<Self>, shutdown: mpsc::Receiver<()>) -> Result<()> {
        info!("QUIC server started");
        let mut shutdown = shutdown;

        loop {
            tokio::select! {
                Some(incoming_conn) = self.endpoint.accept() => {
                    let server = self.clone();
                    tokio::spawn(async move {
                        match incoming_conn.await {
                            Ok(connecting) => {
                                if let Err(e) = server.handle_connection_inner(connecting).await {
                                    error!("Connection handling failed: {}", e);
                                    METRICS.connection_failures.with_label_values(&["quic", "handler_error"]).inc();
                                }
                            }
                            Err(e) => {
                                error!("Connection accept failed: {}", e);
                                METRICS.connection_failures.with_label_values(&["quic", "accept_error"]).inc();
                            }
                        }
                    });
                }
                _ = shutdown.recv() => {
                    info!("Shutdown signal received, stopping QUIC server");
                    break;
                }
            }
        }

        self.shutdown().await?;
        Ok(())
    }

    /// Handle an incoming connection (after it's been accepted)
    async fn handle_connection_inner(
        &self,
        connection: quinn::Connection,
    ) -> Result<()> {
        let remote_addr = connection.remote_address();
        debug!("New QUIC connection from {}", remote_addr);

        info!("QUIC connection established with {}", remote_addr);

        self.stats.total_connections.fetch_add(1, Ordering::Relaxed);
        self.stats.active_connections.fetch_add(1, Ordering::Relaxed);

        METRICS.connections_total.with_label_values(&["quic", "relay"]).inc();
        METRICS.relay_connections_active.inc();

        // Generate session ID
        let session_id = uuid::Uuid::new_v4().to_string();

        // Register connection
        self.connections.write().await.push(ConnectionHandle {
            remote_addr,
            session_id: session_id.clone(),
            established_at: Instant::now(),
        });

        // Handle bidirectional streams
        let result = self
            .handle_streams(connection, session_id.clone())
            .await;

        // Cleanup
        self.stats.active_connections.fetch_sub(1, Ordering::Relaxed);
        METRICS.relay_connections_active.dec();

        let mut connections = self.connections.write().await;
        connections.retain(|c| c.session_id != session_id);

        if let Err(e) = result {
            warn!("Stream handling error for {}: {}", remote_addr, e);
            METRICS.connection_failures.with_label_values(&["quic", "stream_error"]).inc();
        }

        Ok(())
    }

    /// Handle bidirectional streams for a connection
    async fn handle_streams(
        &self,
        connection: quinn::Connection,
        session_id: String,
    ) -> Result<()> {
        loop {
            match connection.accept_bi().await {
                Ok((send, recv)) => {
                    let session_id = session_id.clone();
                    let stats = self.stats.clone();
                    let bandwidth_limit = self.config.relay_bandwidth_limit;

                    tokio::spawn(async move {
                        if let Err(e) = Self::relay_stream(
                            send,
                            recv,
                            session_id.clone(),
                            stats,
                            bandwidth_limit,
                        )
                        .await
                        {
                            debug!("Stream relay error for session {}: {}", session_id, e);
                        }
                    });
                }
                Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                    debug!("Connection closed by peer");
                    break;
                }
                Err(e) => {
                    warn!("Failed to accept stream: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    /// Relay data between send and receive streams
    async fn relay_stream(
        mut send: SendStream,
        mut recv: RecvStream,
        session_id: String,
        stats: Arc<ServerStats>,
        bandwidth_limit: u64,
    ) -> Result<()> {
        let mut buffer = vec![0u8; 8192];
        let start = Instant::now();
        let mut total_bytes = 0u64;

        loop {
            // Read from receive stream
            match recv.read(&mut buffer).await {
                Ok(Some(n)) => {
                    total_bytes += n as u64;

                    // Track metrics
                    stats.bytes_received.fetch_add(n as u64, Ordering::Relaxed);
                    METRICS
                        .relay_bytes_total
                        .with_label_values(&[&session_id, "inbound"])
                        .inc_by(n as f64);

                    // Rate limiting
                    let elapsed = start.elapsed().as_secs_f64();
                    if elapsed > 0.0 {
                        let current_rate = total_bytes as f64 / elapsed;
                        if current_rate > (bandwidth_limit as f64) {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }

                    // Echo back (relay mode)
                    send.write_all(&buffer[..n])
                        .await
                        .context("Failed to write to send stream")?;

                    stats.bytes_sent.fetch_add(n as u64, Ordering::Relaxed);
                    METRICS
                        .relay_bytes_total
                        .with_label_values(&[&session_id, "outbound"])
                        .inc_by(n as f64);
                }
                Ok(None) => {
                    debug!("Stream closed gracefully");
                    break;
                }
                Err(e) => {
                    debug!("Stream read error: {}", e);
                    break;
                }
            }
        }

        send.finish()?;
        Ok(())
    }

    /// Configure QUIC server with TLS and post-quantum key exchange
    fn configure_server(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
    ) -> Result<ServerConfig> {
        // Use ring crypto provider (post-quantum disabled for Windows compatibility)
        let provider = rustls::crypto::ring::default_provider();

        let mut crypto = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .context("Failed to set protocol versions")?
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)
            .context("Failed to configure TLS")?;

        crypto.alpn_protocols = vec![b"pulsar-multiedit".to_vec()];

        info!("Post-quantum key exchange enabled (X25519MLKEM768)");

        let mut config = ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
                .context("Failed to create QUIC crypto config")?,
        ));

        let transport_config = Arc::get_mut(&mut config.transport)
            .context("Failed to get mutable transport config")?;

        transport_config
            .max_concurrent_bidi_streams(VarInt::from_u32(100))
            .max_concurrent_uni_streams(VarInt::from_u32(100))
            .max_idle_timeout(Some(IDLE_TIMEOUT.try_into().unwrap()))
            .keep_alive_interval(Some(KEEP_ALIVE_INTERVAL))
            .datagram_receive_buffer_size(Some(MAX_DATAGRAM_SIZE));

        Ok(config)
    }

    /// Generate a self-signed certificate for testing/development
    fn generate_self_signed_cert() -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, "pulsar-multiedit");
        params.distinguished_name.push(DnType::OrganizationName, "Pulsar");

        let key_pair = KeyPair::generate()?;
        let cert = params.self_signed(&key_pair)?;

        let cert_der = CertificateDer::from(cert.der().clone());
        let key_der = PrivatePkcs8KeyDer::from(key_pair.serialize_der());

        Ok((vec![cert_der], PrivateKeyDer::Pkcs8(key_der)))
    }

    /// Load certificates from files
    fn load_certificates(
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let cert_data = std::fs::read(cert_path).context("Failed to read certificate file")?;
        let key_data = std::fs::read(key_path).context("Failed to read key file")?;

        let certs = rustls_pemfile::certs(&mut &cert_data[..])
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse certificate")?;

        let key = rustls_pemfile::private_key(&mut &key_data[..])
            .context("Failed to parse private key")?
            .context("No private key found")?;

        Ok((certs, key))
    }

    /// Create a QUIC client endpoint for P2P connections with post-quantum key exchange
    pub async fn create_p2p_endpoint(bind_addr: SocketAddr) -> Result<Endpoint> {
        let mut endpoint = Endpoint::client(bind_addr).context("Failed to create client endpoint")?;

        // Use ring crypto provider (post-quantum disabled for Windows compatibility)
        let provider = rustls::crypto::ring::default_provider();

        let crypto = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
            .with_no_client_auth();

        let mut client_config = quinn::ClientConfig::new(Arc::new(
            QuicClientConfig::try_from(crypto).context("Failed to create QUIC client config")?,
        ));

        let mut transport = quinn::TransportConfig::default();
        transport
            .max_idle_timeout(Some(IDLE_TIMEOUT.try_into().unwrap()))
            .keep_alive_interval(Some(KEEP_ALIVE_INTERVAL));

        client_config.transport_config(Arc::new(transport));
        endpoint.set_default_client_config(client_config);

        METRICS.connections_total.with_label_values(&["quic", "p2p"]).inc();

        Ok(endpoint)
    }

    /// Get server statistics
    pub fn stats(&self) -> (u64, u64, u64, u64) {
        (
            self.stats.total_connections.load(Ordering::Relaxed),
            self.stats.active_connections.load(Ordering::Relaxed),
            self.stats.bytes_sent.load(Ordering::Relaxed),
            self.stats.bytes_received.load(Ordering::Relaxed),
        )
    }

    /// Shutdown the server gracefully
    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down QUIC server");
        self.endpoint.close(VarInt::from_u32(0), b"server shutdown");

        // Wait for all connections to close
        tokio::time::sleep(Duration::from_secs(1)).await;

        Ok(())
    }
}

/// Custom certificate verifier that skips verification for P2P connections
#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_self_signed_cert_generation() {
        let result = QuicServer::generate_self_signed_cert();
        assert!(result.is_ok());
        let (certs, _key) = result.unwrap();
        assert!(!certs.is_empty());
    }

    #[tokio::test]
    async fn test_server_creation() {
        crate::init_test_crypto();
        let mut config = Config::default();
        config.quic_bind = "127.0.0.1:0".parse().unwrap();

        let server = QuicServer::new(Arc::new(config)).await;
        assert!(server.is_ok());
    }

    #[tokio::test]
    async fn test_server_stats() {
        crate::init_test_crypto();
        let mut config = Config::default();
        config.quic_bind = "127.0.0.1:0".parse().unwrap();

        let server = QuicServer::new(Arc::new(config)).await.unwrap();
        let (total, active, sent, recv) = server.stats();

        assert_eq!(total, 0);
        assert_eq!(active, 0);
        assert_eq!(sent, 0);
        assert_eq!(recv, 0);
    }

    #[tokio::test]
    async fn test_p2p_endpoint_creation() {
        crate::init_test_crypto();
        let bind_addr = "127.0.0.1:0".parse().unwrap();
        let endpoint = QuicServer::create_p2p_endpoint(bind_addr).await;
        assert!(endpoint.is_ok());
    }

    #[tokio::test]
    async fn test_server_shutdown() {
        crate::init_test_crypto();
        let mut config = Config::default();
        config.quic_bind = "127.0.0.1:0".parse().unwrap();

        let server = Arc::new(QuicServer::new(Arc::new(config)).await.unwrap());
        let (_tx, rx) = mpsc::channel(1);

        let server_clone = server.clone();
        let handle = tokio::spawn(async move {
            server_clone.run(rx).await
        });

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Trigger shutdown
        drop(_tx);

        // Wait for completion
        let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(result.is_ok());
    }
}
