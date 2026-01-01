use anyhow::Result;
use opentelemetry::{global, KeyValue};
use opentelemetry_sdk::{runtime, Resource};
use opentelemetry_otlp::WithExportConfig;
use tracing_subscriber::layer::SubscriberExt;

use crate::config::Config;

/// Initialize OpenTelemetry tracing with OTLP exporter
pub fn init_telemetry(config: &Config) -> Result<()> {
    if let Some(otlp_endpoint) = &config.otlp_endpoint {
        tracing::debug!(
            endpoint = %otlp_endpoint,
            "Initializing OpenTelemetry OTLP exporter"
        );

        let provider = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint(otlp_endpoint),
            )
            .with_trace_config(
                opentelemetry_sdk::trace::Config::default()
                    .with_resource(Resource::new(vec![
                        KeyValue::new("service.name", "pulsar-multiedit"),
                        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                    ])),
            )
            .install_batch(runtime::Tokio)?;

        global::set_tracer_provider(provider);

        tracing::debug!("OpenTelemetry initialized successfully");
    } else {
        tracing::debug!("OpenTelemetry disabled (no OTLP endpoint configured)");
    }

    Ok(())
}

/// Shutdown telemetry gracefully
pub async fn shutdown() {
    tracing::debug!("Shutting down telemetry");
    global::shutdown_tracer_provider();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_telemetry_without_endpoint() {
        let config = Config::default();
        // Should not fail when OTLP endpoint is not configured
        let result = init_telemetry(&config);
        assert!(result.is_ok());
    }
}
