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

// --- Global Allocator Setup ---
use gpui::AppContext;
use ui_log_viewer::TrackingAllocator;

#[global_allocator]
static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

// Re-export render from backend where it actually lives
pub use engine_backend::subsystems::render;
// Re-export graph from ui crate (canonical location)
pub use ui::graph;
// Re-export themes from ui crate (where it belongs)
pub use ui::themes;
// Re-export engine state
pub use engine_state;
// Re-export Combined Assets (includes icons from WGPUI-Component + engine assets)
pub use assets::Assets;
// Re-export OpenSettings from ui crate
pub use ui::OpenSettings;

// --- External and engine imports ---
use crate::settings::EngineSettings;

// --- Internal modules ---

// Re-export core modules that UI needs
pub mod appdata; // App data and resource directory management
pub mod args; // Command-line argument parsing
pub mod assets; // Asset embedding and management
pub mod consts; // Engine constants (name, version, authors, etc.)
pub mod discord; // Discord Rich Presence integration
pub mod file_association;
pub mod gpu_policy; // GPU detection and policy enforcement
pub mod init; // Initialization dependency graph
pub mod logging; // Logging setup and configuration
pub mod macos_permissions;
pub mod runtime; // Async runtime setup and management
pub mod settings; // Engine settings loading and saving
pub mod uri; // URI scheme handling // Project file association management

// --- Engine context re-exports ---
pub use engine_state::{
    EngineContext,
    // sender/receiver removed as messaging is no longer used
    LaunchContext,
    WindowRequest,
};

use init::{task_ids::*, InitContext, InitGraph, InitTask};

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static NvOptimusEnablement: u32 = 0x0000_0001;

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static AmdPowerXpressRequestHighPerformance: u32 = 0x0000_0001;

/// Anti-debugging check for Windows builds.
/// Detects if a debugger is attached and warns the user.
/// This is a basic deterrent — a skilled reverser can bypass it,
/// but it stops casual debugging and automated analysis.
#[cfg(target_os = "windows")]
fn check_debugger_attached() {
    use windows::Win32::System::Diagnostics::Debug::IsDebuggerPresent;
    unsafe {
        if IsDebuggerPresent().as_bool() {
            tracing::warn!(
                "⚠️  Debugger detected! This is a release build — attach only for legitimate debugging."
            );
            // Optionally, the engine could refuse to run under a debugger:
            // std::process::exit(1);
            // For now we just warn, since legitimate debugging should still work.
        }
    }
}

/// Anti-debugging stub for non-Windows platforms.
#[cfg(not(target_os = "windows"))]
fn check_debugger_attached() {
    // On Linux/macOS, ptrace-based detection can be added here.
    // For now, this is a no-op placeholder.
}

/// Main entry point for the Pulsar Engine binary.
///
/// Uses dependency graph-based initialization for explicit ordering and validation.
fn main() {
    // Anti-debugging check — runs before any other initialization.
    check_debugger_attached();

    let _ = rustls::crypto::ring::default_provider().install_default();

    gpu_policy::enforce_discrete_gpu_policy_or_exit();

    macos_permissions::ensure_accessibility_permission_blocking();

    // Name the main thread FIRST
    profiling::set_thread_name("Main Thread");

    // Enable profiling globally
    profiling::enable_profiling();

    // Parse arguments first (needed for init context)
    dotenv::dotenv();
    let parsed = args::parse_args();

    // Create initialization context
    let mut init_ctx = InitContext::new(parsed.clone());

    // Build initialization dependency graph
    let mut graph = InitGraph::new();

    // Task 1: Logging (no dependencies)
    graph
        .add_task(InitTask::new(
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
                tracing::debug!(
                    "🚀 Starting Pulsar Engine with Winit + GPUI Zero-Copy Composition"
                );
                tracing::debug!(
                    "Command-line arguments: {:?}",
                    std::env::args().collect::<Vec<_>>()
                );

                ctx.log_guard = Some(_log_guard);
                Ok(())
            }),
        ))
        .unwrap();

    // Task 2: App data (depends on logging)
    graph
        .add_task(InitTask::new(
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
            }),
        ))
        .unwrap();

    // Task 3: Settings (depends on app data)
    graph
        .add_task(InitTask::new(
            SETTINGS,
            "Settings",
            vec![APPDATA],
            Box::new(|_ctx| {
                let appdata = appdata::setup_appdata();
                tracing::debug!("Loading engine settings from {:?}", appdata.config_file);
                let _engine_settings = EngineSettings::load(&appdata.config_file);
                Ok(())
            }),
        ))
        .unwrap();

    // Task 4: Runtime (depends on logging)
    graph
        .add_task(InitTask::new(
            RUNTIME,
            "Async Runtime",
            vec![LOGGING],
            Box::new(|ctx| {
                let rt = runtime::create_runtime();
                ctx.runtime = Some(rt);
                Ok(())
            }),
        ))
        .unwrap();

    // Task 5: Backend (depends on runtime)
    graph
        .add_task(InitTask::new(
            BACKEND,
            "Engine Backend",
            vec![RUNTIME],
            Box::new(|ctx| {
                let rt = ctx
                    .runtime
                    .as_ref()
                    .ok_or_else(|| init::InitError::MissingContext("Runtime not initialized"))?;

                let backend = rt.block_on(async { engine_backend::EngineBackend::init().await });

                // Set backend as global for access from other parts of the engine
                let backend_arc = std::sync::Arc::new(parking_lot::RwLock::new(backend));
                engine_backend::EngineBackend::set_global(backend_arc);

                // NOTE: Backend is now globally accessible via EngineBackend::global()
                // No need to store in InitContext
                Ok(())
            }),
        ))
        .unwrap();

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
    graph
        .add_task(InitTask::new(
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
            }),
        ))
        .unwrap();

    // Task 7b: Dev Detection (depends on engine context, before set_global)
    graph
        .add_task(InitTask::new(
            DEV_DETECT,
            "Dev/Source Detection",
            vec![ENGINE_CONTEXT],
            Box::new(|ctx| {
                let engine_context = ctx.engine_context.as_ref().ok_or_else(|| {
                    init::InitError::MissingContext("Engine context not initialized")
                })?;

                let dev = engine_state::DevContext::detect();
                if dev.is_source_build {
                    tracing::info!(
                        "Source build detected — workspace root: {:?}",
                        dev.source_path
                    );
                } else {
                    tracing::debug!("Running from installed/distributed binary");
                }
                *engine_context.dev.write() = dev;

                // Stash the embedded default level bytes so the level editor can
                // seed new projects without depending on the engine crate directly.
                if let Some(file) = Assets::get("default.level") {
                    tracing::info!("Embedded default.level found ({} bytes)", file.data.len());
                    *engine_context.default_level_bytes.write() = Some(file.data.into_owned());
                } else {
                    tracing::debug!("No embedded default.level — new projects start empty");
                }

                Ok(())
            }),
        ))
        .unwrap();

    // Task 8: Set Global (depends on dev detection)
    graph
        .add_task(InitTask::new(
            SET_GLOBAL,
            "Set Global Context",
            vec![DEV_DETECT],
            Box::new(|ctx| {
                let engine_context = ctx.engine_context.as_ref().ok_or_else(|| {
                    init::InitError::MissingContext("Engine context not initialized")
                })?;

                engine_context.clone().set_global();
                Ok(())
            }),
        ))
        .unwrap();

    // Task 9: Discord (depends on set_global)
    graph
        .add_task(InitTask::new(
            DISCORD,
            "Discord Rich Presence",
            vec![SET_GLOBAL],
            Box::new(|ctx| {
                let engine_context = ctx.engine_context.as_ref().ok_or_else(|| {
                    init::InitError::MissingContext("Engine context not initialized")
                })?;

                if let Err(e) = discord::init_discord(engine_context, consts::DISCORD_APP_ID) {
                    tracing::warn!("Failed to initialize Discord Rich Presence: {}", e);
                }
                Ok(())
            }),
        ))
        .unwrap();

    // Task 10: URI Registration (depends on runtime)
    graph
        .add_task(InitTask::new(
            URI_REGISTRATION,
            "URI Scheme Registration",
            vec![RUNTIME],
            Box::new(|ctx| {
                let rt = ctx
                    .runtime
                    .as_ref()
                    .ok_or_else(|| init::InitError::MissingContext("Runtime not initialized"))?;

                rt.spawn(async {
                    if let Err(e) = uri::ensure_uri_scheme_registered() {
                        tracing::error!("Failed to register URI scheme: {}", e);
                    }
                });
                Ok(())
            }),
        ))
        .unwrap();

    // Task 11: Project file association prompt (depends on global context)
    graph
        .add_task(InitTask::new(
            FILE_ASSOCIATION,
            "Project File Association",
            vec![SET_GLOBAL],
            Box::new(|_ctx| {
                file_association::maybe_prompt_project_file_association();
                Ok(())
            }),
        ))
        .unwrap();

    // Execute the initialization graph
    if let Err(e) = graph.execute(&mut init_ctx) {
        tracing::error!("Engine initialization failed: {}", e);
        std::process::exit(1);
    }

    // Extract initialized components
    let engine_context = init_ctx
        .engine_context
        .expect("Engine context should be initialized");

    // Run the main event loop via GPUI's `App::run` API.
    profiling::profile_scope!("Engine::EventLoop");

    // create and run GPUI application
    let gpui_app = gpui::Application::new().with_assets(Assets);

    gpui_app.run(move |cx: &mut gpui::App| {
        use ui_common::PulsarWindowExt as _;

        cx.activate(true);
        ui::init(cx);
        ui::themes::init(cx);
        ui_core::init(cx);

        {
            use window_manager::{WindowManager, WindowRegistry};
            cx.set_global(WindowManager::new());
            cx.set_global(WindowRegistry::new());
        }

        // Runs every inventory::submit! registrant from all linked crates automatically.
        window_manager::register_all_windows(cx);

        let mut launch = engine_context.launch.write();

        if let Some(path) = launch.uri_project_path.take() {
            tracing::info!("Opening project splash from URI: {}", path.display());
            open_via_loading_screen(path, cx);
        } else {
            tracing::info!("Opening main entry window");
            let ec = engine_context.clone();
            match engine_context.create_window(
                WindowRequest::Entry,
                window_manager::WindowConfig::entry(),
                move |window, cx| {
                    use gpui::UpdateGlobal as _;

                    let project_cb: std::sync::Arc<
                        dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync,
                    > = std::sync::Arc::new(|path, cx| open_via_loading_screen(path, cx));

                    let git_cb: std::sync::Arc<
                        dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync,
                    > = std::sync::Arc::new(|_path, cx| {
                        window_manager::WindowRegistry::update_global(cx, |reg, cx| {
                            reg.open("GitManagerWindow", cx)
                        });
                    });

                    let settings_cb: std::sync::Arc<dyn Fn(&mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(|cx| {
                            window_manager::WindowRegistry::update_global(cx, |reg, cx| {
                                reg.open("SettingsWindow", cx)
                            });
                        });

                    let fab_cb: std::sync::Arc<dyn Fn(&mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(|cx| {
                            window_manager::WindowRegistry::update_global(cx, |reg, cx| {
                                reg.open("FabSearchWindow", cx)
                            });
                        });

                    ui_entry::create_entry_component(
                        window,
                        cx,
                        &ec,
                        0,
                        project_cb,
                        git_cb,
                        settings_cb,
                        fab_cb,
                    )
                },
                cx,
            ) {
                Ok((wid, _)) => tracing::info!("Entry window opened successfully id={}", wid),
                Err(e) => tracing::error!("Failed to open entry window: {}", e),
            }
        }
    });
}

/// Open a project through the loading-screen splash, then transition to the editor.
///
/// Single canonical path for opening an editor window — URI-launch and entry-screen
/// project-open both go through here.
fn open_via_loading_screen(path: std::path::PathBuf, cx: &mut gpui::App) {
    use ui_common::PulsarWindowExt as _;
    let on_complete: std::sync::Arc<dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync> =
        std::sync::Arc::new(|path, cx| {
            ui_core::PulsarRoot::open(path, cx);
        });
    ui_loading_screen::LoadingScreen::open((path, on_complete), cx);
}
