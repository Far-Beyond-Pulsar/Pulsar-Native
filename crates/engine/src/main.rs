#![allow(warnings)]

// Engine modules and imports
use crate::settings::EngineSettings;
use directories::ProjectDirs;
use gpui::Action;
use gpui::*;
use gpui::SharedString;
use ui::{ scroll::ScrollbarShow, Root };
use ui_core::ToggleCommandPalette;
use serde::Deserialize;
use std::fs;
use std::path::Path;

// Winit imports
use raw_window_handle::{ HasWindowHandle, RawWindowHandle };
use std::collections::HashSet;
use std::sync::{ Arc, Mutex };
use std::sync::mpsc::{ channel, Sender, Receiver };
use std::time::{ Duration, Instant };
use winit::application::ApplicationHandler;
use winit::event::{ ElementState, MouseButton as WinitMouseButton, WindowEvent };
use winit::event_loop::{ ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy };
use winit::window::WindowId;
use winit::keyboard::{ PhysicalKey, KeyCode };

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{ Direct3D::*, Direct3D11::*, Direct3D::Fxc::*, Dxgi::{ Common::*, * } },
    },
};

// Use the library
use pulsar_engine::*;

// Binary-only modules
mod window; // Winit integration (Winit + GPUI coordination)
mod uri; // URI scheme handling

// Use engine_state crate
pub use engine_state::{
    EngineState,
    WindowRequest,
    WindowRequestSender,
    WindowRequestReceiver,
    window_request_channel,
};

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
use window::{
    convert_mouse_button,
    convert_modifiers,
    SimpleClickState,
    MotionSmoother,
    WindowState,
    WinitGpuiApp,
};

fn main() {
    // Initialize logging backend with env filter support
    // Loads .env if present, then checks RUST_LOG from env or .env, or falls back to default
    dotenv::dotenv().ok();
    use tracing_subscriber::fmt::{
        format::FormatEvent,
        format::FormatFields,
        format::Writer,
        FmtContext,
    };
    use serde_json;
    use tracing_subscriber::fmt;
    use tracing_subscriber::registry::LookupSpan;
    use tracing::Subscriber;
    struct GorgeousFormatter;
    impl<S, N> FormatEvent<S, N>
        for GorgeousFormatter
        where S: Subscriber + for<'a> LookupSpan<'a>, N: for<'a> FormatFields<'a> + 'static
    {
        fn format_event(
            &self,
            ctx: &FmtContext<'_, S, N>,
            mut writer: Writer<'_>,
            event: &tracing::Event<'_>
        ) -> std::fmt::Result {
            use std::fmt::Write as _;
            let meta = event.metadata();
            let level = *meta.level();
            let now = chrono::Local::now();
            // Elegant, dark-friendly, harmonious colors
            let (level_str, level_color) = match level {
                tracing::Level::ERROR => ("ERROR", "\x1b[1;91m"), // Bold Red
                tracing::Level::WARN => ("WARN ", "\x1b[1;93m"), // Bold Yellow
                tracing::Level::INFO => ("INFO ", "\x1b[1;94m"), // Bold Blue
                tracing::Level::DEBUG => ("DEBUG", "\x1b[1;92m"), // Bold Green
                tracing::Level::TRACE => ("TRACE", "\x1b[1;95m"), // Bold Magenta
            };
            // Timestamp: dim cyan
            write!(writer, "\x1b[2;36m{}\x1b[0m ", now.format("%Y-%m-%d %H:%M:%S"))?;
            // Level: bold, colored, padded
            write!(writer, "{}{}\x1b[0m ", level_color, level_str)?;
            // Thread ID: dim magenta
            #[cfg(feature = "std")]
            {
                let thread = std::thread::current();
                let thread_id = format!("{:?}", thread.id());
                write!(writer, "\x1b[2;35m[{}]\x1b[0m ", thread_id)?;
            }
            // Target: dim yellow, underlined
            write!(writer, "\x1b[4;2;33m{}\x1b[0m: ", meta.target())?;

            // Capture the message into a string using a visitor
            struct MsgVisitor(String);
            impl tracing_subscriber::field::Visit for MsgVisitor {
                fn record_debug(
                    &mut self,
                    _field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug
                ) {
                    if !self.0.is_empty() {
                        self.0.push(' ');
                    }
                    use std::fmt::Write;
                    let _ = write!(self.0, "{:?}", value);
                }
                fn record_str(&mut self, _field: &tracing::field::Field, value: &str) {
                    if !self.0.is_empty() {
                        self.0.push(' ');
                    }
                    self.0.push_str(value);
                }
            }
            let mut visitor = MsgVisitor(String::new());
            event.record(&mut visitor);
            let msg_buf = visitor.0.trim();

            // Try to pretty-print and colorize JSON if possible, even if embedded
            let mut highlighted = false;
            if let Some(start) = msg_buf.find(|c| (c == '{' || c == '[')) {
                let (prefix, json_candidate) = msg_buf.split_at(start);
                if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(json_candidate) {
                    // Print prefix as normal
                    write!(writer, " {}\n", prefix.trim_end())?;
                    fn color_json(val: &serde_json::Value, buf: &mut String, indent: usize) {
                        match val {
                            serde_json::Value::Object(map) => {
                                buf.push_str("{\n");
                                let len = map.len();
                                for (i, (k, v)) in map.iter().enumerate() {
                                    buf.push_str(&"  ".repeat(indent + 1));
                                    let _ = write!(buf, "\x1b[36m\"{}\"\x1b[0m: ", k);
                                    color_json(v, buf, indent + 1);
                                    if i + 1 != len {
                                        buf.push(',');
                                    }
                                    buf.push('\n');
                                }
                                buf.push_str(&"  ".repeat(indent));
                                buf.push('}');
                            }
                            serde_json::Value::Array(arr) => {
                                buf.push_str("[\n");
                                let len = arr.len();
                                for (i, v) in arr.iter().enumerate() {
                                    buf.push_str(&"  ".repeat(indent + 1));
                                    color_json(v, buf, indent + 1);
                                    if i + 1 != len {
                                        buf.push(',');
                                    }
                                    buf.push('\n');
                                }
                                buf.push_str(&"  ".repeat(indent));
                                buf.push(']');
                            }
                            serde_json::Value::String(s) => {
                                let _ = write!(buf, "\x1b[32m\"{}\"\x1b[0m", s); // Green
                            }
                            serde_json::Value::Number(n) => {
                                let _ = write!(buf, "\x1b[33m{}\x1b[0m", n); // Yellow
                            }
                            serde_json::Value::Bool(b) => {
                                let _ = write!(buf, "\x1b[35m{}\x1b[0m", b); // Magenta
                            }
                            serde_json::Value::Null => {
                                buf.push_str("\x1b[90mnull\x1b[0m"); // Bright black
                            }
                        }
                    }
                    let mut json_buf = String::new();
                    color_json(&json_val, &mut json_buf, 0);
                    write!(writer, "{}", json_buf)?;
                    highlighted = true;
                }
            }
            if !highlighted {
                // Not JSON, or no JSON found, print as normal
                write!(writer, " {}", msg_buf)?;
            }
            writeln!(writer)
        }
    }


    // --- Logging directory setup ---
    use chrono::Local;
    use std::fs;
    use std::path::PathBuf;
    let proj_dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine").expect("Could not determine app data directory");
    let appdata_dir = proj_dirs.data_dir();
    let logs_dir = appdata_dir.join("logs");
    if let Err(e) = fs::create_dir_all(&logs_dir) {
        tracing::error!("[Engine] Failed to create logs directory: {e}");
    }
    let now = Local::now();
    let log_folder = logs_dir.join(format!("{}", now.format("%Y-%m-%d_%H-%M-%S")));
    if let Err(e) = fs::create_dir_all(&log_folder) {
        tracing::error!("[Engine] Failed to create log timestamp folder: {e}");
    }
    let engine_log_path = log_folder.join("engine.log");
    let game_log_path = log_folder.join("game.log");

    // File appender for engine.log
    let engine_log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&engine_log_path)
        .expect("Failed to open engine.log for writing");
    let (non_blocking, _engine_log_guard) = tracing_appender::non_blocking(engine_log_file);
    // IMPORTANT: Keep _engine_log_guard alive for the entire program duration!

    // Optionally, set up game_log_file similarly if needed
    // let game_log_file = std::fs::OpenOptions::new()
    //     .create(true)
    //     .append(true)
    //     .open(&game_log_path)
    //     .expect("Failed to open game.log for writing");
    // let (game_non_blocking, _game_guard) = tracing_appender::non_blocking(game_log_file);

    // Set up tracing subscriber with file output (engine.log) and console
    use tracing_subscriber::prelude::*;
    let rust_log = std::env::var("RUST_LOG").ok();
    let env_filter = match rust_log {
        Some(val) => tracing_subscriber::EnvFilter::new(val),
        None => tracing_subscriber::EnvFilter::new("info,wgpu_hal=warn,wgpu_core=warn,naga=warn"),
    };
    // File log: plain formatting, no ANSI/color codes
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true);

    // Check for -v or --verbose flag in args
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer);

    if verbose {
        // Console log: keep GorgeousFormatter (with color)
        let console_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_target(true)
            .with_thread_ids(true)
            .event_format(GorgeousFormatter);
        registry.with(console_layer).init();
    } else {
        registry.init();
    }

    tracing::debug!("{}", ENGINE_NAME);
    tracing::debug!("Version: {}", ENGINE_VERSION);
    tracing::debug!("Authors: {}", ENGINE_AUTHORS);
    tracing::debug!("Description: {}", ENGINE_DESCRIPTION);
    tracing::debug!("🚀 Starting Pulsar Engine with Winit + GPUI Zero-Copy Composition");

    // Parse command-line arguments for URI launch
    tracing::debug!("Command-line arguments: {:?}", std::env::args().collect::<Vec<_>>());
    let uri_command = match uri::parse_launch_args() {
        Ok(cmd) => {
            if cmd.is_some() {
                tracing::debug!("✅ Successfully parsed URI command: {:?}", cmd);
            }
            cmd
        }
        Err(e) => {
            tracing::warn!("❌ Failed to parse URI arguments: {}", e);
            None
        }
    };

    // Determine app data directory
    let proj_dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine").expect(
        "Could not determine app data directory"
    );
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
    tracing::debug!("Loading engine settings from {:?}", config_file);
    let _engine_settings = EngineSettings::load(&config_file);

    // Initialize Tokio runtime for engine backend
    let rt = tokio::runtime::Builder
        ::new_multi_thread()
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
        tracing::debug!("Launching project from URI: {}", path.display());
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
            Ok(_) => tracing::debug!("✅ Discord Rich Presence initialized"),
            Err(e) => tracing::warn!("⚠️  Discord Rich Presence failed to initialize: {}", e),
        }
    } else {
        tracing::debug!("ℹ️  Discord Rich Presence not configured (set discord_app_id in main.rs)");
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

    // Keep the log guard alive until the very end
    drop(_engine_log_guard);
}
