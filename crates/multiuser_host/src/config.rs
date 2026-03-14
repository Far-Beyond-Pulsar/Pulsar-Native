use clap::Parser;
use std::path::PathBuf;

/// Pulsar Host — dedicated multi-user project server for Pulsar Engine studios.
#[derive(Parser, Debug, Clone)]
#[command(name = "pulsar-host", version, about)]
pub struct Cli {
    /// TCP port to listen on.
    #[arg(long, env = "PULSAR_HOST_PORT", default_value = "7700")]
    pub port: u16,

    /// Bind address.
    #[arg(long, env = "PULSAR_HOST_BIND", default_value = "0.0.0.0")]
    pub bind: String,

    /// Directory used to store project files and metadata.
    #[arg(long, env = "PULSAR_HOST_DATA_DIR", default_value = "./pulsar-host-data")]
    pub data_dir: PathBuf,

    /// Human-readable name shown to connected Pulsar clients.
    #[arg(long, env = "PULSAR_HOST_NAME", default_value = "Pulsar Host Server")]
    pub server_name: String,

    /// Bearer token required for write operations (and reads if set).
    /// Leave empty for an open / development server.
    #[arg(long, env = "PULSAR_HOST_AUTH_TOKEN", default_value = "")]
    pub auth_token: String,

    /// Maximum number of projects this server will host.
    #[arg(long, env = "PULSAR_HOST_MAX_PROJECTS", default_value = "100")]
    pub max_projects: usize,

    /// Log filter directive, e.g. "info", "debug", "pulsar_host=trace".
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    pub log: String,
}

/// Validated runtime configuration derived from CLI args.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub bind: String,
    pub data_dir: PathBuf,
    pub server_name: String,
    /// SHA-256 hex of the raw token, or `None` if auth is disabled.
    pub auth_token_hash: Option<String>,
    pub max_projects: usize,
}

impl Config {
    pub fn from_cli(cli: Cli) -> anyhow::Result<Self> {
        use sha2::{Digest, Sha256};

        std::fs::create_dir_all(&cli.data_dir)?;

        let auth_token_hash = if cli.auth_token.is_empty() {
            None
        } else {
            let mut hasher = Sha256::new();
            hasher.update(cli.auth_token.as_bytes());
            Some(hex::encode(hasher.finalize()))
        };

        Ok(Config {
            port: cli.port,
            bind: cli.bind,
            data_dir: cli.data_dir,
            server_name: cli.server_name,
            auth_token_hash,
            max_projects: cli.max_projects,
        })
    }

    /// Verify a raw bearer token against the stored hash.
    pub fn verify_token(&self, raw: &str) -> bool {
        use sha2::{Digest, Sha256};
        match &self.auth_token_hash {
            None => true, // Auth disabled — all requests pass.
            Some(expected) => {
                let mut hasher = Sha256::new();
                hasher.update(raw.as_bytes());
                hex::encode(hasher.finalize()) == *expected
            }
        }
    }

    /// Returns `true` if authentication is required for read endpoints.
    pub fn auth_required(&self) -> bool {
        self.auth_token_hash.is_some()
    }
}
