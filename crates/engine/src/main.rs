//#![windows_subsystem = "windows"]
//! # Pulsar Engine Main Entry Point
//!
//! ## Initialization Architecture
//!
//! The engine uses a **dependency graph-based initialization system** (`InitGraph`) that:
//! - Explicitly declares dependencies between initialization tasks
//! - Validates the dependency graph (detects cycles, missing dependencies)
//! - Executes tasks in topological order
//! - Provides comprehensive profiling instrumentation
//!
//! ## Key Systems
//!
//! - **Typed Context System** (`EngineContext`) - Type-safe state management
//! - **Dependency Graph Init** (`InitGraph`) - Declarative startup ordering
//! - **Window System** (now internal to GPUI) - windows are managed by GPUI; the engine
//!   no longer provides its own multi-window handler
//! - **Profiling** - Per-task timing and performance analysis
//!
//! ## Initialization Tasks
//!
//! 1. **Logging** - Tracy/tracing setup
//! 2. **App Data** - Config directory initialization
//! 3. **Settings** - Load engine configuration
//! 4. **Runtime** - Tokio async runtime
//! 5. **Backend** - Engine backend subsystems (physics, etc.)
//! 6. **Channels** - (deprecated; windows opened directly via GPUI)
//! 7. **Engine Context** - Global typed state
//! 8. **Set Global** - Register context globally
//! 9. **Discord** - Rich presence initialization
//! 10. **URI Registration** - Custom URI scheme (pulsar://)
//!
//! Each task is profiled with `Engine::Init::{TaskName}` scope.

use gpui::IntoElement;
// --- Global Allocator Setup ---
use ui_log_viewer::TrackingAllocator;
use std::time::Duration;
use gpui::AppContext;

#[global_allocator]
static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

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
use std::path::PathBuf;

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
// Window integration was previously handled by the `window` module, but
// GPUI now manages its own windows.  The event loop lives in
// `event_loop.rs` and the engine no longer exposes a dedicated window API.
pub mod uri;        // URI scheme handling
pub mod init;       // Initialization dependency graph (Phase 1 - new)

// --- Engine context re-exports ---
pub use engine_state::{
    EngineContext,
    WindowRequest,
    // sender/receiver removed as messaging is no longer used
    LaunchContext,
};

use init::{InitGraph, InitTask, InitContext, task_ids::*};

/// Main entry point for the Pulsar Engine binary.
///
/// Uses dependency graph-based initialization for explicit ordering and validation.
fn main() {
    // Name the main thread FIRST
    profiling::set_thread_name("Main Thread");

    // Enable profiling globally
    profiling::enable_profiling();

    // Parse arguments first (needed for init context)
    dotenv::dotenv().ok();
    let parsed = args::parse_args();

    // Create initialization context
    let mut init_ctx = InitContext::new(parsed.clone());

    // Build initialization dependency graph
    let mut graph = InitGraph::new();

    // Task 1: Logging (no dependencies)
    graph.add_task(InitTask::new(
        LOGGING,
        "Logging",
        vec![],
        Box::new(move |ctx| {
            let _log_guard = logging::init(ctx.launch_args.verbose);

            // Engine metadata logging
            tracing::debug!("{}", consts::ENGINE_NAME);
            tracing::debug!("Version: {}", consts::ENGINE_VERSION);
            tracing::debug!("Authors: {}", consts::ENGINE_AUTHORS);
            tracing::debug!("Description: {}", consts::ENGINE_DESCRIPTION);
            tracing::debug!("🚀 Starting Pulsar Engine with Winit + GPUI Zero-Copy Composition");
            tracing::debug!("Command-line arguments: {:?}", std::env::args().collect::<Vec<_>>());

            ctx.log_guard = Some(_log_guard);
            Ok(())
        })
    )).unwrap();

    // Task 2: App data (depends on logging)
    graph.add_task(InitTask::new(
        APPDATA,
        "App Data",
        vec![LOGGING],
        Box::new(|_ctx| {
            let appdata = appdata::setup_appdata();
            tracing::debug!("App data directory: {:?}", appdata.appdata_dir);
            tracing::debug!("Themes directory: {:?}", appdata.themes_dir);
            tracing::debug!("Config directory: {:?}", appdata.config_dir);
            tracing::debug!("Config file: {:?}", appdata.config_file);
            Ok(())
        })
    )).unwrap();

    // Task 3: Settings (depends on app data)
    graph.add_task(InitTask::new(
        SETTINGS,
        "Settings",
        vec![APPDATA],
        Box::new(|_ctx| {
            let appdata = appdata::setup_appdata();
            tracing::debug!("Loading engine settings from {:?}", appdata.config_file);
            let _engine_settings = EngineSettings::load(&appdata.config_file);
            Ok(())
        })
    )).unwrap();

    // Task 4: Runtime (depends on logging)
    graph.add_task(InitTask::new(
        RUNTIME,
        "Async Runtime",
        vec![LOGGING],
        Box::new(|ctx| {
            let rt = runtime::create_runtime();
            ctx.runtime = Some(rt);
            Ok(())
        })
    )).unwrap();

    // Task 5: Backend (depends on runtime)
    graph.add_task(InitTask::new(
        BACKEND,
        "Engine Backend",
        vec![RUNTIME],
        Box::new(|ctx| {
            let rt = ctx.runtime.as_ref().ok_or_else(||
                init::InitError::MissingContext("Runtime not initialized")
            )?;

            let backend = rt.block_on(async {
                engine_backend::EngineBackend::init().await
            });

            // Set backend as global for access from other parts of the engine
            let backend_arc = std::sync::Arc::new(parking_lot::RwLock::new(backend));
            engine_backend::EngineBackend::set_global(backend_arc);

            // NOTE: Backend is now globally accessible via EngineBackend::global()
            // No need to store in InitContext
            Ok(())
        })
    )).unwrap();

    // Task 6: Channels (no dependencies)
    // (disabled – window management will be done directly via GPUI)
    /*graph.add_task(InitTask::new(
        CHANNELS,
        "Window Channels",
        vec![],
        Box::new(|ctx| {
            let (window_tx, window_rx) = channel::<WindowRequest>();
            ctx.window_tx = Some(window_tx);
            ctx.window_rx = Some(window_rx);
            Ok(())
        })
    )).unwrap();*/

    // Task 7: Engine Context (depends on channels)
    graph.add_task(InitTask::new(
        ENGINE_CONTEXT,
        "Engine Context",
        vec![], // no dependency now
        Box::new(|ctx| {
            let engine_context = EngineContext::new();

            // Handle URI project path if present
            if let Some(uri::UriCommand::OpenProject { path }) = &ctx.launch_args.uri_command {
                tracing::debug!("Launching project from URI: {}", path.display());
                let mut launch = engine_context.launch.write();
                launch.uri_project_path = Some(path.clone());
            }

            ctx.engine_context = Some(engine_context);
            Ok(())
        })
    )).unwrap();

    // Task 8: Set Global (depends on engine context)
    graph.add_task(InitTask::new(
        SET_GLOBAL,
        "Set Global Context",
        vec![ENGINE_CONTEXT],
        Box::new(|ctx| {
            let engine_context = ctx.engine_context.as_ref().ok_or_else(||
                init::InitError::MissingContext("Engine context not initialized")
            )?;

            engine_context.clone().set_global();
            Ok(())
        })
    )).unwrap();

    // Task 9: Discord (depends on set_global)
    graph.add_task(InitTask::new(
        DISCORD,
        "Discord Rich Presence",
        vec![SET_GLOBAL],
        Box::new(|ctx| {
            let engine_context = ctx.engine_context.as_ref().ok_or_else(||
                init::InitError::MissingContext("Engine context not initialized")
            )?;

            if let Err(e) = discord::init_discord(engine_context, consts::DISCORD_APP_ID) {
                tracing::warn!("Failed to initialize Discord Rich Presence: {}", e);
            }
            Ok(())
        })
    )).unwrap();

    // Task 10: URI Registration (depends on runtime)
    graph.add_task(InitTask::new(
        URI_REGISTRATION,
        "URI Scheme Registration",
        vec![RUNTIME],
        Box::new(|ctx| {
            let rt = ctx.runtime.as_ref().ok_or_else(||
                init::InitError::MissingContext("Runtime not initialized")
            )?;

            rt.spawn(async {
                if let Err(e) = uri::ensure_uri_scheme_registered() {
                    tracing::error!("Failed to register URI scheme: {}", e);
                }
            });
            Ok(())
        })
    )).unwrap();

    // Execute the initialization graph
    if let Err(e) = graph.execute(&mut init_ctx) {
        tracing::error!("Engine initialization failed: {}", e);
        std::process::exit(1);
    }

    // Extract initialized components
    let engine_context = init_ctx.engine_context.expect("Engine context should be initialized");

    // Run the main event loop via GPUI's `App::run` API.
    profiling::profile_scope!("Engine::EventLoop");

    // create and run GPUI application
    let gpui_app = gpui::Application::new().with_assets(Assets);

    gpui_app.run(move |cx: &mut gpui::App| {
        cx.activate(true);
        ui::init(cx);
        // ensure themes registry is initialized and state.json applied
        ui::themes::init(cx);

        // decide initial window once the UI context is ready
        let mut launch = engine_context.launch.write();
        {
            if let Some(path) = launch.uri_project_path.take() {
                tracing::info!("Opening project splash from URI: {}", path.display());
                let wid = engine_context.next_window_id();
                engine_context.register_window(wid, engine_state::WindowContext::new(wid, WindowRequest::ProjectSplash { project_path: path.to_string_lossy().to_string() }));
                let pathbuf = PathBuf::from(path);
                let opts = make_window_options(
                    Some("Pulsar Engine"),
                    gpui::point(gpui::px(120.0), gpui::px(120.0)),
                    gpui::size(gpui::px(900.0), gpui::px(600.0)),
                    None,
                );
                match cx.open_window(opts, move |window, cx| {
                    ui_loading_screen::create_loading_component(pathbuf.clone(), wid, window, cx)
                }) {
                    Ok(_) => tracing::info!("Splash window opened successfully"),
                    Err(e) => tracing::error!("Failed to open splash window: {:?}", e),
                }
            } else {
                tracing::info!("Opening main entry window");
                let wid = engine_context.next_window_id();
                engine_context.register_window(wid, engine_state::WindowContext::new(wid, WindowRequest::Entry));
                let ec = engine_context.clone();
                let opts = make_window_options(
                    Some("Pulsar Engine"),
                    gpui::point(gpui::px(100.0), gpui::px(100.0)),
                    gpui::size(gpui::px(800.0), gpui::px(600.0)),
                    Some(gpui::Size { width: gpui::px(600.), height: gpui::px(400.) }),
                );
                match cx.open_window(opts, move |window, cx| {
                    // callbacks capture environment by value; cx is passed when invoked
                    let ec_clone = ec.clone();
                    let project_cb: std::sync::Arc<dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(move |pathbuf, cx| {
                            // open splash window from engine logic
                            let ec2 = ec_clone.clone();
                            let wid2 = ec2.next_window_id();
                            ec2.register_window(wid2, engine_state::WindowContext::new(wid2, WindowRequest::ProjectSplash { project_path: pathbuf.to_string_lossy().to_string() }));
                            let opts = make_window_options(
                                Some("Pulsar Engine"),
                                gpui::point(gpui::px(120.0), gpui::px(120.0)),
                                gpui::size(gpui::px(900.0), gpui::px(600.0)),
                                None,
                            );
                            if let Err(e) = cx.open_window(opts, move |window, cx| {
                                ui_loading_screen::create_loading_component(pathbuf.clone(), wid2, window, cx)
                            }) {
                                tracing::error!("failed to open splash window: {:?}", e);
                            }
                        });

                    let ec_clone2 = ec.clone();
                    let git_cb: std::sync::Arc<dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(move |pathbuf, cx| {
                            let ec3 = ec_clone2.clone();
                            let wid3 = ec3.next_window_id();
                            ec3.register_window(wid3, engine_state::WindowContext::new(wid3, WindowRequest::GitManager { project_path: pathbuf.to_string_lossy().to_string() }));
                            let opts = make_window_options(
                                Some("Git Manager"),
                                gpui::point(gpui::px(150.0), gpui::px(150.0)),
                                gpui::size(gpui::px(800.0), gpui::px(600.0)),
                                None,
                            );
                            if let Err(e) = cx.open_window(opts, move |window, cx| {
                                ui_git_manager::create_git_manager_component(window, cx, pathbuf.clone())
                            }) {
                                tracing::error!("failed to open git manager window: {:?}", e);
                            }
                        });
                    // callback for opening settings window from entry screen
                    let ec_clone3 = ec.clone();
                    let settings_cb: std::sync::Arc<dyn Fn(&mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(move |cx| {
                            let ec4 = ec_clone3.clone();
                            let wid4 = ec4.next_window_id();
                            ec4.register_window(wid4, engine_state::WindowContext::new(wid4, WindowRequest::Settings));
                            let opts = make_window_options(
                                Some("Settings"),
                                gpui::point(gpui::px(150.0), gpui::px(150.0)),
                                gpui::size(gpui::px(700.0), gpui::px(500.0)),
                                None,
                            );
                            if let Err(e) = cx.open_window(opts, move |window, cx| {
                                ui_settings::create_settings_component(window, cx, &ec4.clone())
                            }) {
                                tracing::error!("failed to open settings window: {:?}", e);
                            }
                        });

                    ui_entry::create_entry_component(window, cx, &ec, wid, project_cb, git_cb, settings_cb)
                }) {
                    Ok(_) => tracing::info!("Entry window opened successfully"),
                    Err(e) => tracing::error!("Failed to open entry window: {:?}", e),
                }
            }
        }
    });
}

/// Build common `WindowOptions` to reduce boilerplate.
fn make_window_options(
    title: Option<&'static str>,
    origin: gpui::Point<gpui::Pixels>,
    win_size: gpui::Size<gpui::Pixels>,
    min_size: Option<gpui::Size<gpui::Pixels>>,
) -> gpui::WindowOptions {
    gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(origin, win_size))),
        titlebar: None,
        kind: gpui::WindowKind::Normal,
        is_resizable: true,
        window_decorations: Some(gpui::WindowDecorations::Client),
        window_min_size: min_size,
        ..Default::default()
    }
}

