//! Pulsar MultiEdit Service - Main Entry Point
//!
//! This binary starts the complete Pulsar MultiEdit service including:
//! - HTTP admin server (health, metrics, session management)
//! - QUIC relay server
//! - UDP hole punching coordinator
//! - WebSocket rendezvous signaling
//! - Session garbage collection
//! - Telemetry and metrics collection

use anyhow::{Context, Result};
use colored::Colorize;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::mpsc;
use tracing::info;

use pulsar_multiedit::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Check if we should run the simple test server instead
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--test-server" {
        tracing::debug!("🧪 Running simple test server on 0.0.0.0:8080");
        tracing::debug!("This is a minimal stdlib TCP server for testing external connectivity");
        return simple_test_server::run();
    }

    // 1. Load configuration
    let config = Arc::new(Config::from_env().context("Failed to load configuration")?);

    // 2. Initialize logging and tracing
    logging::init(&config).context("Failed to initialize logging")?;

    // 3. Print beautiful banner
    logging::print_banner(&config);

    // 4. Log configuration (sanitized)
    logging::log_config(&config);

    info!("{} Starting all services...", "🚀".to_string());

    // 5. Initialize telemetry (OpenTelemetry)
    if config.otlp_endpoint.is_some() {
        info!("📡 Initializing telemetry...");
        telemetry::init_telemetry(&config).context("Failed to initialize telemetry")?;
        logging::log_status("📡", "Telemetry", "READY", true);
    }

    // 6. Initialize metrics registry
    info!("📊 Initializing metrics...");
    let _metrics = metrics::init(&config).context("Failed to initialize metrics")?;
    logging::log_status("📊", "Metrics", "READY", true);

    // 7. Initialize persistence (DB + local storage)
    if config.database_url.is_some() || config.storage_dir.is_some() {
        info!("💾 Initializing persistence layer...");
        let _persistence = PersistenceLayer::new((*config).clone())
            .await
            .context("Failed to initialize persistence layer")?;
        logging::log_status("💾", "Persistence", "READY", true);
    }

    // 8. Create shutdown channels
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    let (http_shutdown_tx, http_shutdown_rx) = mpsc::channel::<()>(1);
    let (quic_shutdown_tx, quic_shutdown_rx) = mpsc::channel::<()>(1);

    // 9. Start HTTP admin server
    info!("🌐 Starting HTTP server on {}...", config.http_bind);
    let config_http = config.clone();
    let http_handle = tokio::spawn(async move {
        if let Err(e) = http_server::run_server(config_http, http_shutdown_rx).await {
            tracing::error!("❌ HTTP server failed: {}", e);
        }
    });
    logging::log_status("🌐", "HTTP Server", "LISTENING", true);

    // 10. Start QUIC relay server
    info!("⚡ Starting QUIC relay on {}...", config.quic_bind);
    let config_quic = config.clone();
    let quic_handle = tokio::spawn(async move {
        match QuicServer::new(config_quic).await {
            Ok(server) => {
                let server = Arc::new(server);
                if let Err(e) = server.run(quic_shutdown_rx).await {
                    tracing::error!("❌ QUIC server failed: {}", e);
                }
            }
            Err(e) => {
                tracing::error!("❌ Failed to create QUIC server: {}", e);
            }
        }
    });
    logging::log_status("⚡", "QUIC Relay", "LISTENING", true);

    // 11. Start session garbage collector
    info!("🧹 Starting session garbage collector...");
    let config_gc = config.clone();
    let sessions = Arc::new(SessionStore::new(config_gc.clone()));
    let sessions_gc = sessions.clone();
    let gc_handle = tokio::spawn(async move {
        session::garbage_collector_loop(sessions_gc, Duration::from_secs(60)).await;
    });
    logging::log_status("🧹", "Garbage Collector", "RUNNING", true);

    tracing::debug!("\n{}", "✅ All services started successfully!".bright_green().bold());
    tracing::debug!("{}\n", "━".repeat(60).bright_black());

    // 12. Wait for shutdown signal
    tokio::select! {
        _ = signal::ctrl_c() => {
            tracing::debug!("\n{}", "⚠️  Received Ctrl+C, initiating graceful shutdown...".bright_yellow());
        }
        _ = shutdown_rx.recv() => {
            info!("🛑 Received shutdown signal");
        }
    }

    // 13. Initiate graceful shutdown
    tracing::debug!("{}", "🛑 Shutting down services...".bright_yellow().bold());
    tracing::debug!("{}", "━".repeat(60).bright_black());

    // Signal all services to stop
    let _ = http_shutdown_tx.send(()).await;
    let _ = quic_shutdown_tx.send(()).await;

    // Wait for services with timeout
    let shutdown_timeout = Duration::from_secs(10);

    tokio::select! {
        _ = http_handle => logging::log_status("🌐", "HTTP Server", "STOPPED", true),
        _ = tokio::time::sleep(shutdown_timeout) => {
            tracing::warn!("⚠️  HTTP server shutdown timeout");
        }
    }

    tokio::select! {
        _ = quic_handle => logging::log_status("⚡", "QUIC Relay", "STOPPED", true),
        _ = tokio::time::sleep(shutdown_timeout) => {
            tracing::warn!("⚠️  QUIC server shutdown timeout");
        }
    }

    // Stop GC task (it runs indefinitely)
    gc_handle.abort();
    logging::log_status("🧹", "Garbage Collector", "STOPPED", true);

    // 14. Shutdown telemetry
    if config.otlp_endpoint.is_some() {
        telemetry::shutdown().await;
        logging::log_status("📡", "Telemetry", "STOPPED", true);
    }

    tracing::debug!("\n{}", "👋 Pulsar MultiEdit service stopped cleanly".bright_green().bold());
    tracing::debug!("{}\n", "━".repeat(60).bright_black());

    Ok(())
}
