mod api;
mod auth;
mod config;
mod projects;
mod sessions;
mod state;

use anyhow::Result;
use clap::Parser;
use tracing::info;

use config::{Cli, Config};
use projects::ProjectManager;
use state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialise structured logging.
    tracing_subscriber::fmt()
        .with_env_filter(&cli.log)
        .with_target(false)
        .compact()
        .init();

    info!("Starting Pulsar Host v{}", env!("CARGO_PKG_VERSION"));

    // Build validated config.
    let config = Config::from_cli(cli)?;
    let data_dir = config.data_dir.clone();

    info!("Server name  : {}", config.server_name);
    info!("Bind address : {}:{}", config.bind, config.port);
    info!("Data directory: {}", data_dir.display());
    info!("Auth required : {}", config.auth_required());
    info!("Max projects  : {}", config.max_projects);

    // Load projects from disk.
    let projects = ProjectManager::load(data_dir)?;
    info!("{} project(s) loaded", projects.count());

    // Build shared application state.
    let state = AppState::new(config.clone(), projects);

    // Assemble the Axum router.
    let app = api::router(state).layer(tower_http::trace::TraceLayer::new_for_http());

    // Start listening.
    let addr = format!("{}:{}", config.bind, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Pulsar Host ready on http://{}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
