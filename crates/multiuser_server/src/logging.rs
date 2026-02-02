//! Pretty logging with colors and structured formatting
//!
//! This module provides beautiful, human-readable logs with:
//! - Color-coded log levels
//! - Structured field formatting
//! - Timestamps with millisecond precision
//! - Context-aware prefixes
//! - Compact yet informative output

use anyhow::Result;
use colored::Colorize;
use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

use crate::config::Config;

/// Initialize the logging subsystem with pretty, colored output
///
/// This sets up a beautiful logging system with:
/// - ANSI color support for log levels
/// - Structured field display
/// - Timestamp with millisecond precision
/// - Target module names
/// - Span tracking for async operations
pub fn init(config: &Config) -> Result<()> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    // Pretty formatted logs with colors
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false)
        .with_level(true)
        .with_ansi(true)
        .compact()
        .with_span_events(FmtSpan::CLOSE);

    // Build the subscriber
    Registry::default()
        .with(env_filter)
        .with(fmt_layer)
        .try_init()?;

    Ok(())
}

/// Print a beautiful startup banner with configuration details
pub fn print_banner(config: &Config) {
    let banner = format!(
        r#"
╔══════════════════════════════════════════════════════════════╗
║                                                              ║
║   {}  {}   ║
║                                                              ║
║   {}                                          ║
║                                                              ║
╚══════════════════════════════════════════════════════════════╝
"#,
        "🚀 Pulsar MultiEdit Server".bright_cyan().bold(),
        format!("v{}", env!("CARGO_PKG_VERSION")).bright_yellow(),
        "Production-grade multiplayer editing".bright_white()
    );

    tracing::debug!("{}", banner);
}

/// Display configuration in a pretty formatted way
pub fn log_config(config: &Config) {
    tracing::debug!("\n{}", "📋 Configuration".bright_cyan().bold());
    tracing::debug!("{}", "━".repeat(60).bright_black());
    
    log_config_item("HTTP Server", &format!("{}", config.http_bind), "🌐");
    log_config_item("QUIC Relay", &format!("{}", config.quic_bind), "⚡");
    log_config_item("UDP Hole Punch", &format!("{}", config.udp_bind), "🔌");
    log_config_item("Max Sessions", &format!("{}", config.max_sessions), "👥");
    log_config_item(
        "Bandwidth Limit",
        &format!("{}/s", format_bytes(config.relay_bandwidth_limit)),
        "📊"
    );
    log_config_item(
        "Session TTL",
        &format!("{}s", config.session_ttl.as_secs()),
        "⏱️"
    );
    log_config_item("Log Level", &config.log_level.to_uppercase(), "📝");
    
    if let Some(db_url) = &config.database_url {
        let sanitized = sanitize_connection_string(db_url);
        log_config_item("Database", &sanitized, "💾");
    }

    if let Some(storage_dir) = &config.storage_dir {
        log_config_item("Local Storage", storage_dir, "📁");
    }

    if config.tls_cert_path.is_some() {
        log_config_item("TLS", "Enabled (custom cert)", "🔒");
    } else {
        log_config_item("TLS", "Self-signed certificate", "🔓");
    }
    
    if let Some(endpoint) = &config.otlp_endpoint {
        log_config_item("Telemetry", endpoint, "📡");
    }
    
    tracing::debug!("{}\n", "━".repeat(60).bright_black());
}

/// Log a single configuration item with emoji and formatting
fn log_config_item(label: &str, value: &str, emoji: &str) {
    tracing::debug!(
        "  {} {:<18} {}",
        emoji,
        label.bright_white().bold(),
        value.bright_green()
    );
}

/// Format bytes in human-readable format (KB, MB, GB)
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Sanitize database connection strings to hide passwords
fn sanitize_connection_string(conn_str: &str) -> String {
    if let Some(at_pos) = conn_str.find('@') {
        if let Some(protocol_end) = conn_str.find("://") {
            let protocol = &conn_str[..protocol_end + 3];
            let rest = &conn_str[at_pos..];
            return format!("{}***:***{}", protocol, rest);
        }
    }
    "***".to_string()
}

/// Log a status message with an icon
pub fn log_status(icon: &str, message: &str, status: &str, is_success: bool) {
    let colored_status = if is_success {
        status.bright_green().bold()
    } else {
        status.bright_red().bold()
    };
    
    tracing::debug!("{} {} → {}", icon, message, colored_status);
}

/// Log a metric with formatting
pub fn log_metric(icon: &str, label: &str, value: &str) {
    tracing::debug!(
        "{} {} = {}",
        icon,
        label.bright_cyan(),
        value.bright_yellow().bold()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_sanitize_connection_string() {
        let conn = "postgresql://user:password@localhost:5432/db";
        let sanitized = sanitize_connection_string(conn);
        assert!(!sanitized.contains("password"));
        assert!(sanitized.contains("@localhost"));
    }
}
