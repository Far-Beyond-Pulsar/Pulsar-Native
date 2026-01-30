//! # Pulsar MultiEdit - Production-grade multiplayer editing server
//!
//! A comprehensive collaborative editing service that provides:
//!
//! ## Features
//!
//! - **WebSocket Signaling** - Real-time peer coordination and messaging
//! - **QUIC Relay** - High-performance encrypted relay for P2P connections
//! - **Session Management** - Create, join, and manage collaborative sessions
//! - **Authentication** - JWT-based auth with role management
//! - **Bandwidth Control** - Per-session rate limiting and monitoring
//! - **Health Checks** - Kubernetes-ready liveness and readiness probes
//! - **Metrics** - Prometheus metrics for observability
//! - **Telemetry** - OpenTelemetry tracing support
//! - **Persistence** - PostgreSQL + S3 for session snapshots
//!
//! ## Architecture
//!
//! The server consists of several independent services:
//!
//! - **HTTP Server** - REST API + WebSocket signaling (port 8080)
//! - **QUIC Relay** - Encrypted relay for peer traffic (port 8443)
//! - **UDP Punch** - NAT traversal coordinator (port 7000)
//! - **Metrics** - Prometheus endpoint (port 9090)
//!
//! ## Quick Start
//!
//! ```bash
//! # Start with defaults
//! cargo run --bin pulsar-multiedit
//!
//! # With custom configuration
//! cargo run --bin pulsar-multiedit -- --http-bind 0.0.0.0:8080 --log-level debug
//! ```
//!
//! ## Module Overview
//!
//! - [`config`] - Configuration management
//! - [`logging`] - Pretty logging with colors
//! - [`http_server`] - HTTP API and WebSocket signaling
//! - [`session`] - Session lifecycle management
//! - [`relay`] - QUIC relay server
//! - [`auth`] - Authentication and authorization
//! - [`metrics`] - Prometheus metrics
//! - [`health`] - Health check endpoints
//! - [`telemetry`] - OpenTelemetry integration
//! - [`persistence`] - Database and S3 storage

pub mod simple_test_server;
pub mod auth;
pub mod config;
pub mod crdt;
pub mod health;
pub mod http_server;
pub mod logging;
pub mod metrics;
pub mod nat;
pub mod persistence;
pub mod relay;
pub mod rendezvous;
pub mod session;
pub mod shutdown;
pub mod telemetry;
pub mod transport;

// Re-export commonly used types
pub use auth::{AuthService, Role};
pub use config::Config;
pub use crdt::{ORSet, RGASeq};
pub use health::HealthChecker;
pub use metrics::METRICS;
pub use persistence::PersistenceLayer;
pub use session::SessionStore;
pub use transport::{QuicServer, TcpSimultaneousOpen, UdpHolePuncher};

#[cfg(test)]
pub(crate) fn init_test_crypto() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Use ring provider (post-quantum disabled for Windows compatibility)
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
