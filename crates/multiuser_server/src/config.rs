//! Configuration management for Pulsar MultiEdit Server
//!
//! This module handles loading configuration from multiple sources:
//! - Environment variables (highest priority)
//! - Command-line arguments
//! - Configuration files (JSON or TOML)
//! - Sensible defaults
//!
//! Configuration validation ensures all required parameters are set correctly.

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tracing::warn;

/// Pulsar MultiEdit Service Configuration
///
/// All settings can be configured via:
/// - Environment variables (prefixed with `PULSAR_`)
/// - Command-line flags
/// - Config file (JSON or TOML format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// HTTP admin server bind address
    pub http_bind: SocketAddr,

    /// QUIC relay bind address
    pub quic_bind: SocketAddr,

    /// UDP port for hole punching
    pub udp_bind: SocketAddr,

    /// TLS certificate path
    pub tls_cert_path: Option<PathBuf>,

    /// TLS private key path
    pub tls_key_path: Option<PathBuf>,

    /// Database connection URL
    pub database_url: Option<String>,

    /// Local storage directory for snapshots (S3 removed)
    pub storage_dir: Option<String>,

    /// Maximum concurrent sessions
    pub max_sessions: usize,

    /// Maximum relay bandwidth per session (bytes/sec)
    pub relay_bandwidth_limit: u64,

    /// NAT probe timeout
    pub nat_probe_timeout: Duration,

    /// Hole punch timeout
    pub hole_punch_timeout: Duration,

    /// Session TTL (time to live)
    pub session_ttl: Duration,

    /// Prometheus metrics port
    pub prometheus_port: u16,

    /// OpenTelemetry OTLP endpoint
    pub otlp_endpoint: Option<String>,

    /// Log level
    pub log_level: String,

    /// JWT secret for signing tokens
    pub jwt_secret: String,

    /// Server Ed25519 private key (base64 encoded)
    pub server_ed25519_key: Option<String>,

    /// Enable mTLS for admin APIs
    pub mtls_enabled: bool,

    /// Client CA certificate path for mTLS
    pub client_ca_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            http_bind: "0.0.0.0:8080".parse().unwrap(),
            quic_bind: "0.0.0.0:8443".parse().unwrap(),
            udp_bind: "0.0.0.0:7000".parse().unwrap(),
            tls_cert_path: None,
            tls_key_path: None,
            database_url: None,
            storage_dir: None,
            max_sessions: 10000,
            relay_bandwidth_limit: 10 * 1024 * 1024, // 10 MB/s
            nat_probe_timeout: Duration::from_secs(5),
            hole_punch_timeout: Duration::from_secs(10),
            session_ttl: Duration::from_secs(3600),
            prometheus_port: 9090,
            otlp_endpoint: None,
            log_level: "info".to_string(),
            jwt_secret: "change-this-secret-in-production".to_string(),
            server_ed25519_key: None,
            mtls_enabled: false,
            client_ca_path: None,
        }
    }
}

/// CLI arguments
#[derive(Parser, Debug)]
#[command(name = "pulsar-multiedit")]
#[command(about = "Pulsar MultiEdit — rendezvous, relay, and matchmaker service")]
pub struct Cli {
    /// Configuration file path
    #[arg(short, long, env = "PULSAR_CONFIG")]
    pub config: Option<PathBuf>,

    /// HTTP bind address
    #[arg(long, env = "PULSAR_HTTP_BIND")]
    pub http_bind: Option<SocketAddr>,

    /// QUIC bind address
    #[arg(long, env = "PULSAR_QUIC_BIND")]
    pub quic_bind: Option<SocketAddr>,

    /// UDP bind address
    #[arg(long, env = "PULSAR_UDP_BIND")]
    pub udp_bind: Option<SocketAddr>,

    /// Database URL
    #[arg(long, env = "PULSAR_DATABASE_URL")]
    pub database_url: Option<String>,

    /// Local storage directory for snapshots
    #[arg(long, env = "PULSAR_STORAGE_DIR")]
    pub storage_dir: Option<String>,

    /// Log level
    #[arg(long, env = "PULSAR_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// JWT secret
    #[arg(long, env = "PULSAR_JWT_SECRET")]
    pub jwt_secret: Option<String>,

    /// OTLP endpoint
    #[arg(long, env = "PULSAR_OTLP_ENDPOINT")]
    pub otlp_endpoint: Option<String>,

    /// TLS certificate path
    #[arg(long, env = "PULSAR_TLS_CERT")]
    pub tls_cert: Option<PathBuf>,

    /// TLS private key path
    #[arg(long, env = "PULSAR_TLS_KEY")]
    pub tls_key: Option<PathBuf>,

    /// Server Ed25519 private key (base64)
    #[arg(long, env = "PULSAR_SERVER_ED25519_KEY")]
    pub server_ed25519_key: Option<String>,
}

impl Config {
    /// Load configuration from environment, CLI args, and optional config file
    pub fn from_env() -> Result<Self> {
        let cli = Cli::parse();
        let mut config = Self::default();

        // Load from config file if provided
        if let Some(config_path) = &cli.config {
            let config_str = std::fs::read_to_string(config_path)
                .context("Failed to read config file")?;
            let file_config: Config = serde_json::from_str(&config_str)
                .or_else(|_| toml::from_str(&config_str))
                .context("Failed to parse config file")?;
            config = file_config;
        }

        // Override with CLI args
        if let Some(http_bind) = cli.http_bind {
            config.http_bind = http_bind;
        }
        if let Some(quic_bind) = cli.quic_bind {
            config.quic_bind = quic_bind;
        }
        if let Some(udp_bind) = cli.udp_bind {
            config.udp_bind = udp_bind;
        }
        if let Some(database_url) = cli.database_url {
            config.database_url = Some(database_url);
        }
        if let Some(storage_dir) = cli.storage_dir {
            config.storage_dir = Some(storage_dir);
        }
        if let Some(jwt_secret) = cli.jwt_secret {
            config.jwt_secret = jwt_secret;
        }
        if let Some(otlp_endpoint) = cli.otlp_endpoint {
            config.otlp_endpoint = Some(otlp_endpoint);
        }
        if let Some(tls_cert) = cli.tls_cert {
            config.tls_cert_path = Some(tls_cert);
        }
        if let Some(tls_key) = cli.tls_key {
            config.tls_key_path = Some(tls_key);
        }
        if let Some(key) = cli.server_ed25519_key {
            config.server_ed25519_key = Some(key);
        }

        config.log_level = cli.log_level;

        // Validate configuration
        config.validate()?;

        Ok(config)
    }

    /// Validate configuration settings
    ///
    /// Ensures all required parameters are set correctly and warns about
    /// insecure defaults that should be changed in production.
    fn validate(&self) -> Result<()> {
        if self.jwt_secret == "change-this-secret-in-production" {
            warn!("⚠️  Using default JWT secret - CHANGE THIS IN PRODUCTION!");
        }

        if self.max_sessions == 0 {
            anyhow::bail!("❌ max_sessions must be greater than 0");
        }

        if self.mtls_enabled && self.client_ca_path.is_none() {
            anyhow::bail!("❌ mtls_enabled requires client_ca_path to be set");
        }

        if self.tls_cert_path.is_none() {
            warn!("🔓 Using self-signed TLS certificate (not recommended for production)");
        }

        Ok(())
    }
}

// TOML parsing is handled inline in from_env() via toml::from_str

// We need to add toml dependency
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.max_sessions, 10000);
        assert_eq!(config.log_level, "info");
    }

    #[test]
    fn test_config_validation() {
        let mut config = Config::default();
        assert!(config.validate().is_ok());

        config.max_sessions = 0;
        assert!(config.validate().is_err());
    }
}
