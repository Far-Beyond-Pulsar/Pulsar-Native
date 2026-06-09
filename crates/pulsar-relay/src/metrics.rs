use anyhow::Result;
use once_cell::sync::Lazy;
use prometheus::{
    CounterVec, Encoder, Gauge, GaugeVec, HistogramOpts, HistogramVec, Opts, Registry, TextEncoder,
};
use std::sync::Arc;

use crate::config::Config;

/// Global metrics registry
pub static METRICS: Lazy<Arc<Metrics>> =
    Lazy::new(|| Arc::new(Metrics::new().expect("failed to initialize metrics registry")));

/// Metrics collection for Pulsar MultiEdit
pub struct Metrics {
    pub registry: Registry,

    // Session metrics
    pub sessions_total: CounterVec,
    pub sessions_active: Gauge,
    pub sessions_closed: CounterVec,

    // Connection metrics
    pub connections_total: CounterVec,
    pub connection_failures: CounterVec,
    pub p2p_success_ratio: Gauge,

    // Relay metrics
    pub relay_bytes_total: CounterVec,
    pub relay_connections_active: Gauge,
    pub relay_bandwidth_usage: GaugeVec,

    // Hole punching metrics
    pub hole_punch_attempts: CounterVec,
    pub hole_punch_success: CounterVec,
    pub hole_punch_duration: HistogramVec,

    // NAT traversal metrics
    pub nat_type_detected: CounterVec,

    // Rendezvous metrics
    pub signaling_messages: CounterVec,
    pub rendezvous_latency: HistogramVec,

    // HTTP metrics
    pub http_requests: CounterVec,
    pub http_request_duration: HistogramVec,

    // Health metrics
    pub database_health: Gauge,
    pub s3_health: Gauge,
}

impl Metrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let sessions_total = CounterVec::new(
            Opts::new("pulsar_sessions_total", "Total number of sessions created"),
            &["host_id"],
        )?;
        registry.register(Box::new(sessions_total.clone()))?;

        let sessions_active = Gauge::new(
            "pulsar_sessions_active",
            "Number of currently active sessions",
        )?;
        registry.register(Box::new(sessions_active.clone()))?;

        let sessions_closed = CounterVec::new(
            Opts::new("pulsar_sessions_closed", "Total number of sessions closed"),
            &["reason"],
        )?;
        registry.register(Box::new(sessions_closed.clone()))?;

        let connections_total = CounterVec::new(
            Opts::new(
                "pulsar_connections_total",
                "Total number of connection attempts",
            ),
            &["proto", "type"],
        )?;
        registry.register(Box::new(connections_total.clone()))?;

        let connection_failures = CounterVec::new(
            Opts::new(
                "pulsar_connection_failures_total",
                "Total number of connection failures",
            ),
            &["proto", "reason"],
        )?;
        registry.register(Box::new(connection_failures.clone()))?;

        let p2p_success_ratio = Gauge::new(
            "pulsar_p2p_success_ratio",
            "Ratio of successful P2P connections",
        )?;
        registry.register(Box::new(p2p_success_ratio.clone()))?;

        let relay_bytes_total = CounterVec::new(
            Opts::new("pulsar_relay_bytes_total", "Total bytes relayed"),
            &["session_id", "direction"],
        )?;
        registry.register(Box::new(relay_bytes_total.clone()))?;

        let relay_connections_active = Gauge::new(
            "pulsar_relay_connections_active",
            "Number of active relay connections",
        )?;
        registry.register(Box::new(relay_connections_active.clone()))?;

        let relay_bandwidth_usage = GaugeVec::new(
            Opts::new(
                "pulsar_relay_bandwidth_usage_bytes_per_sec",
                "Current relay bandwidth usage per session",
            ),
            &["session_id"],
        )?;
        registry.register(Box::new(relay_bandwidth_usage.clone()))?;

        let hole_punch_attempts = CounterVec::new(
            Opts::new(
                "pulsar_hole_punch_attempts_total",
                "Total number of hole punch attempts",
            ),
            &["nat_type"],
        )?;
        registry.register(Box::new(hole_punch_attempts.clone()))?;

        let hole_punch_success = CounterVec::new(
            Opts::new(
                "pulsar_hole_punch_success_total",
                "Total number of successful hole punches",
            ),
            &["nat_type"],
        )?;
        registry.register(Box::new(hole_punch_success.clone()))?;

        let hole_punch_duration = HistogramVec::new(
            HistogramOpts::new(
                "pulsar_hole_punch_duration_seconds",
                "Time taken for hole punching",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0]),
            &["nat_type"],
        )?;
        registry.register(Box::new(hole_punch_duration.clone()))?;

        let nat_type_detected = CounterVec::new(
            Opts::new("pulsar_nat_type_detected_total", "Total NAT types detected"),
            &["nat_type"],
        )?;
        registry.register(Box::new(nat_type_detected.clone()))?;

        let signaling_messages = CounterVec::new(
            Opts::new(
                "pulsar_signaling_messages_total",
                "Total signaling messages processed",
            ),
            &["message_type"],
        )?;
        registry.register(Box::new(signaling_messages.clone()))?;

        let rendezvous_latency = HistogramVec::new(
            HistogramOpts::new(
                "pulsar_rendezvous_latency_seconds",
                "Rendezvous message latency",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]),
            &["operation"],
        )?;
        registry.register(Box::new(rendezvous_latency.clone()))?;

        let http_requests = CounterVec::new(
            Opts::new("pulsar_http_requests_total", "Total HTTP requests"),
            &["method", "path", "status"],
        )?;
        registry.register(Box::new(http_requests.clone()))?;

        let http_request_duration = HistogramVec::new(
            HistogramOpts::new(
                "pulsar_http_request_duration_seconds",
                "HTTP request duration",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
            &["method", "path"],
        )?;
        registry.register(Box::new(http_request_duration.clone()))?;

        let database_health = Gauge::new(
            "pulsar_database_health",
            "Database health status (1 = healthy, 0 = unhealthy)",
        )?;
        registry.register(Box::new(database_health.clone()))?;

        let s3_health = Gauge::new(
            "pulsar_s3_health",
            "S3 health status (1 = healthy, 0 = unhealthy)",
        )?;
        registry.register(Box::new(s3_health.clone()))?;

        Ok(Self {
            registry,
            sessions_total,
            sessions_active,
            sessions_closed,
            connections_total,
            connection_failures,
            p2p_success_ratio,
            relay_bytes_total,
            relay_connections_active,
            relay_bandwidth_usage,
            hole_punch_attempts,
            hole_punch_success,
            hole_punch_duration,
            nat_type_detected,
            signaling_messages,
            rendezvous_latency,
            http_requests,
            http_request_duration,
            database_health,
            s3_health,
        })
    }

    /// Encode metrics to Prometheus text format
    pub fn encode(&self) -> Result<String> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = vec![];
        encoder.encode(&metric_families, &mut buffer)?;
        Ok(String::from_utf8(buffer)?)
    }

    /// Get metrics as JSON (for dashboards)
    pub fn as_json(&self) -> Result<serde_json::Value> {
        let metric_families = self.registry.gather();
        let metrics: Vec<_> = metric_families
            .iter()
            .map(|family| {
                serde_json::json!({
                    "name": family.get_name(),
                    "help": family.get_help(),
                    "type": format!("{:?}", family.get_field_type()),
                    "metrics": family.get_metric().len(),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "total_metrics": metrics.len(),
            "metrics": metrics,
        }))
    }
}

/// Initialize metrics system
pub fn init(_config: &Config) -> Result<Arc<Metrics>> {
    tracing::debug!("Metrics system initialized");
    Ok(METRICS.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() -> Result<(), prometheus::Error> {
        let metrics = Metrics::new()?;
        assert!(metrics.encode().is_ok());
        Ok(())
    }

    #[test]
    fn test_metrics_json() -> Result<(), Box<dyn std::error::Error>> {
        let metrics = Metrics::new()?;
        let json = metrics.as_json()?;
        assert!(json["total_metrics"].as_u64().is_some_and(|n| n > 0));
        Ok(())
    }

    #[test]
    fn test_session_metrics() -> Result<(), Box<dyn std::error::Error>> {
        let metrics = Metrics::new()?;
        metrics.sessions_total.with_label_values(&["test"]).inc();
        metrics.sessions_active.set(5.0);
        assert!(metrics.encode()?.contains("pulsar_sessions_active 5"));
        Ok(())
    }
}
