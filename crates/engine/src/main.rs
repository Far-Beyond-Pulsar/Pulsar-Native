#![allow(warnings)]

// Engine modules and imports
use crate::settings::EngineSettings;
use directories::ProjectDirs;
use gpui::Action;
use gpui::*;
use gpui::SharedString;
use ui::{scroll::ScrollbarShow, Root};
use ui_core::ToggleCommandPalette;
use serde::Deserialize;
use std::fs;
use std::path::Path;

// Winit imports
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window as WinitWindow, WindowId};
use winit::keyboard::{PhysicalKey, KeyCode};

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Direct3D::Fxc::*,
            Dxgi::{Common::*, *},
        },
    },
};

// Use the library
use pulsar_engine::*;

// Binary-only modules
mod window;  // Winit integration (Winit + GPUI coordination)
mod uri;     // URI scheme handling

// Use engine_state crate
pub use engine_state::{EngineState, WindowRequest, WindowRequestSender, WindowRequestReceiver, window_request_channel};

// Engine constants
pub const ENGINE_NAME: &str = env!("CARGO_PKG_NAME");
pub const ENGINE_LICENSE: &str = env!("CARGO_PKG_LICENSE");
pub const ENGINE_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const ENGINE_HOMEPAGE: &str = env!("CARGO_PKG_HOMEPAGE");
pub const ENGINE_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const ENGINE_DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");
pub const ENGINE_LICENSE_FILE: &str = env!("CARGO_PKG_LICENSE_FILE");

// WindowRequest now comes from engine_state crate

// Engine actions
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = story, no_json)]
pub struct SelectScrollbarShow(ScrollbarShow);

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = story, no_json)]
pub struct SelectLocale(SharedString);

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = story, no_json)]
pub struct SelectFont(usize);

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = story, no_json)]
pub struct SelectRadius(usize);

// Re-export OpenSettings from ui crate
pub use ui::OpenSettings;

// Import window management utilities from the window module
use window::{convert_mouse_button, convert_modifiers, SimpleClickState, MotionSmoother, WindowState, WinitGpuiApp};

fn main() {
    // Initialize logging backend with env filter support
    // Set RUST_LOG=debug to see debug logs, RUST_LOG=trace for all logs
    // Filter out wgpu shader compilation spam by setting wgpu crates to warn level
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,wgpu_hal=warn,wgpu_core=warn,naga=warn"))
        )
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    tracing::info!("{}", ENGINE_NAME);
    tracing::info!("Version: {}", ENGINE_VERSION);
    tracing::info!("Authors: {}", ENGINE_AUTHORS);
    tracing::info!("Description: {}", ENGINE_DESCRIPTION);
    tracing::info!("🚀 Starting Pulsar Engine with Winit + GPUI Zero-Copy Composition");

    // Parse command-line arguments for URI launch
    let uri_command = match uri::parse_launch_args() {
        Ok(cmd) => cmd,
        Err(e) => {
            tracing::warn!("Failed to parse URI arguments: {}", e);
            None
        }
    };

    // Determine app data directory
    let proj_dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .expect("Could not determine app data directory");
    let appdata_dir = proj_dirs.data_dir();
    let themes_dir = appdata_dir.join("themes");
    let config_dir = appdata_dir.join("configs");
    let config_file = config_dir.join("engine.toml");

    tracing::debug!("App data directory: {:?}", appdata_dir);
    tracing::debug!("Themes directory: {:?}", themes_dir);
    tracing::debug!("Config directory: {:?}", config_dir);
    tracing::debug!("Config file: {:?}", config_file);

    // Extract bundled themes if not present
    if !themes_dir.exists() {
        if let Err(e) = fs::create_dir_all(&themes_dir) {
            tracing::error!("Failed to create themes directory: {e}");
        } else {
            // Copy all themes from project themes/ to appdata_dir/themes/
            let project_themes_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("themes");
            if let Ok(entries) = fs::read_dir(&project_themes_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(name) = path.file_name() {
                            let dest = themes_dir.join(name);
                            let _ = fs::copy(&path, &dest);
                        }
                    }
                }
            }
        }
    }

    // Create default config if not present
    if !config_file.exists() {
        if let Err(e) = fs::create_dir_all(&config_dir) {
            tracing::error!("Failed to create config directory: {e}");
        }
        let default_settings = EngineSettings::default();
        default_settings.save(&config_file);
    }

    // Load settings
    tracing::info!("Loading engine settings from {:?}", config_file);
    let _engine_settings = EngineSettings::load(&config_file);

    // Initialize Tokio runtime for engine backend
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .thread_name("PulsarEngineRuntime")
        .enable_all()
        .build()
        .unwrap();

    // Init the Game engine backend (subsystems, etc)
    rt.block_on(engine_backend::EngineBackend::init());

    // Create channel for window creation requests
    let (window_tx, window_rx) = channel::<WindowRequest>();

    // Create shared engine state with window sender
    let engine_state = EngineState::new().with_window_sender(window_tx.clone());

    // Store URI project path if present
    if let Some(uri::UriCommand::OpenProject { path }) = uri_command {
        tracing::info!("Launching project from URI: {}", path.display());
        engine_state.set_metadata(
            "uri_project_path".to_string(),
            path.to_string_lossy().to_string()
        );
    }

    // Set global engine state for access from GPUI views
    engine_state.clone().set_global();

    // Initialize Discord Rich Presence
    // NOTE: Replace this with your Discord Application ID from https://discord.com/developers/applications
    // To disable Discord integration, simply comment out these lines
    let discord_app_id = "1450965386014228491";
    if discord_app_id != "YOUR_DISCORD_APPLICATION_ID_HERE" {
        match engine_state.init_discord(discord_app_id) {
            Ok(_) => tracing::info!("✅ Discord Rich Presence initialized"),
            Err(e) => tracing::warn!("⚠️  Discord Rich Presence failed to initialize: {}", e),
        }
    } else {
        tracing::info!("ℹ️  Discord Rich Presence not configured (set discord_app_id in main.rs)");
    }

    // Register URI scheme with OS (background task)
    // Uses Tokio runtime created earlier
    rt.spawn(async {
        if let Err(e) = uri::ensure_uri_scheme_registered() {
            tracing::error!("Failed to register URI scheme: {}", e);
        }
    });

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    // Use Wait mode for event-driven rendering (only render when needed)
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = WinitGpuiApp::new(engine_state, window_rx);
    event_loop.run_app(&mut app).expect("Failed to run event loop");
}

