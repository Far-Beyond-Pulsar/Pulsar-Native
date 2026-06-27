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

use std::hash::DefaultHasher;

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
mod steps;
pub mod uri; // URI scheme handling // Project file association management

// --- Engine context re-exports ---
pub use engine_state::{
    EngineContext,
    // sender/receiver removed as messaging is no longer used
    LaunchContext,
    WindowRequest,
};

use init::{init_task, task_ids::*, InitContext, InitGraph};

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
    init_task!(graph, LOGGING, "Logging", [], steps::logging::run);

    // Task 2: App data (depends on logging)
    init_task!(graph, APPDATA, "App Data", [LOGGING], steps::appdata::run);

    // Task 3: Settings (depends on app data)
    init_task!(graph, SETTINGS, "Settings", [APPDATA], steps::settings::run);

    // Task 4: Runtime (depends on logging)
    init_task!(
        graph,
        RUNTIME,
        "Async Runtime",
        [LOGGING],
        steps::runtime::run
    );

    // Task 5: Backend (depends on runtime)
    init_task!(
        graph,
        BACKEND,
        "Engine Backend",
        [RUNTIME],
        steps::backend::run
    );

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
    init_task!(
        graph,
        ENGINE_CONTEXT,
        "Engine Context",
        [],
        steps::engine_context::run
    );

    // Task 7b: Dev Detection (depends on engine context, before set_global)
    init_task!(
        graph,
        DEV_DETECT,
        "Dev/Source Detection",
        [ENGINE_CONTEXT],
        steps::dev_detect::run
    );

    // Task 8: Set Global (depends on dev detection)
    init_task!(
        graph,
        SET_GLOBAL,
        "Set Global Context",
        [DEV_DETECT],
        steps::set_global::run
    );

    // Task 9: Discord (depends on set_global)
    init_task!(
        graph,
        DISCORD,
        "Discord Rich Presence",
        [SET_GLOBAL],
        steps::discord::run
    );

    // Task 10: URI Registration (depends on runtime)
    init_task!(
        graph,
        URI_REGISTRATION,
        "URI Scheme Registration",
        [RUNTIME],
        steps::uri_registration::run
    );

    // Task 11: Project file association prompt (depends on global context)
    // init_task!(
    //     graph,
    //     FILE_ASSOCIATION,
    //     "Project File Association",
    //     [SET_GLOBAL],
    //     steps::file_association::run
    // );

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
        let t_gpui = std::time::Instant::now();
        tracing::info!("[GPUI startup] begin");

        cx.activate(true);

        let t = std::time::Instant::now();
        ui::init(cx);
        tracing::info!("[GPUI startup] ui::init {}ms", t.elapsed().as_millis());

        let t = std::time::Instant::now();
        ui::themes::init(cx);
        tracing::info!(
            "[GPUI startup] ui::themes::init {}ms",
            t.elapsed().as_millis()
        );

        let t = std::time::Instant::now();
        ui_core::init(cx);
        tracing::info!("[GPUI startup] ui_core::init {}ms", t.elapsed().as_millis());

        {
            use window_manager::{WindowManager, WindowRegistry};
            cx.set_global(WindowManager::new());
            cx.set_global(WindowRegistry::new());
        }

        // Runs every inventory::submit! registrant from all linked crates automatically.
        let t = std::time::Instant::now();
        window_manager::register_all_windows(cx);
        tracing::info!(
            "[GPUI startup] register_all_windows {}ms",
            t.elapsed().as_millis()
        );

        let uri_path = engine_context
            .store
            .get_or_init::<engine_state::LaunchContext>()
            .update(|l| l.uri_project_path.take());

        tracing::info!(
            "[GPUI startup] pre-window total {}ms",
            t_gpui.elapsed().as_millis()
        );

        if let Some(path) = uri_path {
            tracing::info!("Opening project splash from URI: {}", path.display());
            open_via_loading_screen(path, cx);
        } else {
            tracing::info!("Opening main entry window");
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
