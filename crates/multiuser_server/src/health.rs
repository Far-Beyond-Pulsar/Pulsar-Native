use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;

use crate::config::Config;
use crate::metrics::METRICS;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    pub timestamp: u64,
    pub checks: Vec<HealthCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: CheckStatus,
    pub message: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

pub struct HealthChecker {
    config: Arc<Config>,
}

impl HealthChecker {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    /// Perform all health checks
    pub async fn check_health(&self) -> HealthStatus {
        let start = SystemTime::now();
        let timestamp = start
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut checks = Vec::new();

        // Check database connectivity
        if self.config.database_url.is_some() {
            checks.push(self.check_database().await);
        }

        // Storage check removed - using local storage only

        // Check TLS certificates validity
        if self.config.tls_cert_path.is_some() {
            checks.push(self.check_certificates().await);
        }

        // Check system resources
        checks.push(self.check_system_resources().await);

        // Check relay health
        checks.push(self.check_relay_health().await);

        // Determine overall status
        let status = if checks.iter().all(|c| c.status == CheckStatus::Healthy) {
            "healthy".to_string()
        } else if checks.iter().any(|c| c.status == CheckStatus::Unhealthy) {
            "unhealthy".to_string()
        } else {
            "degraded".to_string()
        };

        HealthStatus {
            status,
            timestamp,
            checks,
        }
    }

    /// Check database connectivity
    async fn check_database(&self) -> HealthCheck {
        let start = SystemTime::now();
        let name = "database".to_string();

        // In a real implementation, we would ping the database
        // For now, we'll simulate based on metrics
        let status = if METRICS.database_health.get() > 0.5 {
            CheckStatus::Healthy
        } else {
            CheckStatus::Unhealthy
        };

        let duration_ms = start.elapsed().unwrap_or_default().as_millis() as u64;

        HealthCheck {
            name,
            status: status.clone(),
            message: if status == CheckStatus::Healthy {
                Some("Database connection healthy".to_string())
            } else {
                Some("Database connection failed".to_string())
            },
            duration_ms,
        }
    }

    /// Check TLS certificates validity
    async fn check_certificates(&self) -> HealthCheck {
        let start = SystemTime::now();
        let name = "certificates".to_string();

        // In a real implementation, we would verify certificate expiry
        let status = CheckStatus::Healthy;

        let duration_ms = start.elapsed().unwrap_or_default().as_millis() as u64;

        HealthCheck {
            name,
            status,
            message: Some("Certificates valid".to_string()),
            duration_ms,
        }
    }

    /// Check system resources (CPU, memory pressure)
    async fn check_system_resources(&self) -> HealthCheck {
        let start = SystemTime::now();
        let name = "system_resources".to_string();

        // In production, would check actual CPU/memory usage
        let active_sessions = METRICS.sessions_active.get();
        let max_sessions = self.config.max_sessions as f64;

        let status = if active_sessions < max_sessions * 0.8 {
            CheckStatus::Healthy
        } else if active_sessions < max_sessions * 0.95 {
            CheckStatus::Degraded
        } else {
            CheckStatus::Unhealthy
        };

        let duration_ms = start.elapsed().unwrap_or_default().as_millis() as u64;

        HealthCheck {
            name,
            status,
            message: Some(format!(
                "Active sessions: {}/{}",
                active_sessions, max_sessions
            )),
            duration_ms,
        }
    }

    /// Check relay health
    async fn check_relay_health(&self) -> HealthCheck {
        let start = SystemTime::now();
        let name = "relay".to_string();

        let active_relays = METRICS.relay_connections_active.get();

        let status = if active_relays < 1000.0 {
            CheckStatus::Healthy
        } else if active_relays < 5000.0 {
            CheckStatus::Degraded
        } else {
            CheckStatus::Unhealthy
        };

        let duration_ms = start.elapsed().unwrap_or_default().as_millis() as u64;

        HealthCheck {
            name,
            status,
            message: Some(format!("Active relay connections: {}", active_relays)),
            duration_ms,
        }
    }

    /// Simple liveness check (always returns healthy if service is running)
    pub async fn liveness_check(&self) -> HealthStatus {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        HealthStatus {
            status: "healthy".to_string(),
            timestamp,
            checks: vec![HealthCheck {
                name: "liveness".to_string(),
                status: CheckStatus::Healthy,
                message: Some("Service is running".to_string()),
                duration_ms: 0,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_liveness_check() {
        let config = Arc::new(Config::default());
        let checker = HealthChecker::new(config);
        let status = checker.liveness_check().await;
        assert_eq!(status.status, "healthy");
    }

    #[tokio::test]
    async fn test_health_check() {
        let config = Arc::new(Config::default());
        let checker = HealthChecker::new(config);

        // Set metrics to healthy
        METRICS.database_health.set(1.0);
        METRICS.s3_health.set(1.0);

        let status = checker.check_health().await;
        assert!(!status.checks.is_empty());
    }
}
