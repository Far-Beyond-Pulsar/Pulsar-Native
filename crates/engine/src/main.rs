#![windows_subsystem = "windows"]
//! Pulsar Engine Main Entry Point
//!
//! This file initializes the engine, sets up logging, loads configuration, handles app data,
//! initializes the runtime, sets up Discord Rich Presence, and launches the main event loop.

// Re-export render from backend where it actually lives
pub use engine_backend::subsystems::render;
// Re-export compiler and graph from ui crate (canonical location)
pub use ui::compiler;
pub use ui::graph;
// Re-export themes from ui crate (where it belongs)
pub use ui::themes;
// Re-export engine state
pub use engine_state;
// Re-export Assets type for convenience
pub use assets::Assets;
// Re-export OpenSettings from ui crate
pub use ui::OpenSettings;

// --- External and engine imports ---
use crate::settings::EngineSettings;
use std::sync::mpsc::channel;

// --- Internal modules ---

// Re-export core modules that UI needs
pub mod assets;     // Asset embedding and management
pub mod settings;   // Engine settings loading and saving
pub mod logging;    // Logging setup and configuration
pub mod args;       // Command-line argument parsing
pub mod appdata;    // App data and resource directory management
pub mod consts;     // Engine constants (name, version, authors, etc.)
pub mod discord;    // Discord Rich Presence integration
pub mod runtime;    // Async runtime setup and management
pub mod event_loop; // Main event loop handling
pub mod window;     // Winit integration (Winit + GPUI coordination)
pub mod uri;        // URI scheme handling

// --- Engine state re-exports ---
pub use engine_state::{
    EngineState,
    WindowRequest,
    WindowRequestSender,
    WindowRequestReceiver,
    window_request_channel,
};

/// Main entry point for the Pulsar Engine binary.
///
/// Responsibilities:
/// - Loads environment variables and logging
/// - Parses command-line arguments
/// - Sets up app data and configuration
/// - Initializes the async runtime and engine backend
/// - Sets up engine state and Discord Rich Presence
/// - Registers URI scheme
/// - Runs the main event loop
fn main() {
    // --- Load environment and initialize logging ---
    dotenv::dotenv().ok();
    let parsed = args::parse_args();
    let _log_guard = logging::init(parsed.verbose);

    // --- Engine metadata logging ---
    tracing::debug!("{}", consts::ENGINE_NAME);
    tracing::debug!("Version: {}", consts::ENGINE_VERSION);
    tracing::debug!("Authors: {}", consts::ENGINE_AUTHORS);
    tracing::debug!("Description: {}", consts::ENGINE_DESCRIPTION);
    tracing::debug!("🚀 Starting Pulsar Engine with Winit + GPUI Zero-Copy Composition");
    tracing::debug!("Command-line arguments: {:?}", std::env::args().collect::<Vec<_>>());

    // --- App data and configuration setup ---
    let appdata = appdata::setup_appdata();
    tracing::debug!("App data directory: {:?}", appdata.appdata_dir);
    tracing::debug!("Themes directory: {:?}", appdata.themes_dir);
    tracing::debug!("Config directory: {:?}", appdata.config_dir);
    tracing::debug!("Config file: {:?}", appdata.config_file);

    // --- Load engine settings ---
    tracing::debug!("Loading engine settings from {:?}", appdata.config_file);
    let _engine_settings = EngineSettings::load(&appdata.config_file);

    // --- Initialize async runtime and engine backend ---
    let rt = runtime::create_runtime();
    rt.block_on(engine_backend::EngineBackend::init());

    // --- Engine state and window channel setup ---
    let (window_tx, window_rx) = channel::<WindowRequest>();
    let engine_state = EngineState::new().with_window_sender(window_tx.clone());

    // --- Handle URI project path if present ---
    if let Some(uri::UriCommand::OpenProject { path }) = parsed.uri_command {
        tracing::debug!("Launching project from URI: {}", path.display());
        engine_state.set_metadata(
            "uri_project_path".to_string(),
            path.to_string_lossy().to_string()
        );
    }

    // --- Set global engine state for GPUI views ---
    engine_state.clone().set_global();

    // --- Initialize Discord Rich Presence ---
    discord::init_discord(&engine_state, consts::DISCORD_APP_ID);

    // --- Register URI scheme with OS (background task) ---
    rt.spawn(async {
        if let Err(e) = uri::ensure_uri_scheme_registered() {
            tracing::error!("Failed to register URI scheme: {}", e);
        }
    });

    // --- Run the main event loop ---
    event_loop::run_event_loop(engine_state, window_rx);

    // --- Keep the log guard alive until the very end ---
    drop(_log_guard);
}
