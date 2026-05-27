use anyhow::Result;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{trace::SdkTracerProvider, Resource};
use std::sync::{Mutex, OnceLock};

use crate::config::Config;

static TRACER_PROVIDER: OnceLock<Mutex<Option<SdkTracerProvider>>> = OnceLock::new();

fn tracer_provider_slot() -> &'static Mutex<Option<SdkTracerProvider>> {
    TRACER_PROVIDER.get_or_init(|| Mutex::new(None))
}

/// Initialize OpenTelemetry tracing with OTLP exporter
pub fn init_telemetry(config: &Config) -> Result<()> {
    if let Some(otlp_endpoint) = &config.otlp_endpoint {
        tracing::debug!(
            endpoint = %otlp_endpoint,
            "Initializing OpenTelemetry OTLP exporter"
        );

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(otlp_endpoint)
            .build()?;
        let provider = SdkTracerProvider::builder()
            .with_resource(
                Resource::builder()
                    .with_attributes(vec![
                    KeyValue::new("service.name", "pulsar-multiedit"),
                    KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                    ])
                    .build(),
            )
            .with_batch_exporter(exporter)
            .build();

        global::set_tracer_provider(provider.clone());
        *tracer_provider_slot().lock().unwrap() = Some(provider);

        tracing::debug!("OpenTelemetry initialized successfully");
    } else {
        tracing::debug!("OpenTelemetry disabled (no OTLP endpoint configured)");
    }

    Ok(())
}

/// Shutdown telemetry gracefully
pub async fn shutdown() {
    tracing::debug!("Shutting down telemetry");
    if let Some(provider) = tracer_provider_slot().lock().unwrap().take() {
        if let Err(error) = provider.shutdown() {
            tracing::warn!(%error, "Failed to shut down OpenTelemetry tracer provider cleanly");
        }
    }
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
