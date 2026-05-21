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
use wgpu::{Backends, DeviceType, Instance, InstanceDescriptor};

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
use file_association_manager::{AssociationError, AssociationRequest, FileAssociationManager};

// --- Internal modules ---

// Re-export core modules that UI needs
pub mod appdata; // App data and resource directory management
pub mod args; // Command-line argument parsing
pub mod assets; // Asset embedding and management
pub mod consts; // Engine constants (name, version, authors, etc.)
pub mod discord; // Discord Rich Presence integration
pub mod logging; // Logging setup and configuration
pub mod macos_permissions;
pub mod runtime;
pub mod settings; // Engine settings loading and saving // Async runtime setup and management
                  // Window integration was previously handled by the `window` module, but
                  // GPUI now manages its own windows.  The event loop lives in
                  // `event_loop.rs` and the engine no longer exposes a dedicated window API.
pub mod init;
pub mod uri; // URI scheme handling // Initialization dependency graph (Phase 1 - new)

// --- Engine context re-exports ---
pub use engine_state::{
    EngineContext,
    // sender/receiver removed as messaging is no longer used
    LaunchContext,
    WindowRequest,
};

use init::{task_ids::*, InitContext, InitGraph, InitTask};

const PROJECT_ASSOC_EXTENSION: &str = "Pulsar.toml";
const PROJECT_ASSOC_MIME: &str = "application/x-pulsar-project";

#[cfg(target_os = "macos")]
const MACOS_BUNDLE_ID: &str = "dev.pulsar.engine";
#[cfg(target_os = "macos")]
const MACOS_ASSOC_QUERY_EXTENSION: &str = "toml";

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static NvOptimusEnablement: u32 = 0x0000_0001;

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static AmdPowerXpressRequestHighPerformance: u32 = 0x0000_0001;

#[derive(Debug, Clone)]
struct GpuPolicyProbe {
    has_discrete_gpu: bool,
    selected_gpu_name: Option<String>,
}

fn probe_gpu_policy() -> GpuPolicyProbe {
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::all(),
        ..Default::default()
    });

    let adapters = futures::executor::block_on(instance.enumerate_adapters(Backends::all()));
    let mut has_discrete_gpu = false;
    let mut selected_gpu_name = None;

    for adapter in adapters {
        let info = adapter.get_info();
        if info.device_type == DeviceType::DiscreteGpu {
            has_discrete_gpu = true;
            if selected_gpu_name.is_none() {
                selected_gpu_name = Some(info.name);
            }
        }
    }

    GpuPolicyProbe {
        has_discrete_gpu,
        selected_gpu_name,
    }
}

fn prompt_continue_without_discrete_gpu() -> bool {
    let description = "No discrete GPU was detected.\n\nPulsar is configured to prefer dGPU for best performance.\n\nContinue anyway using the available GPU?";

    let choice = rfd::MessageDialog::new()
        .set_title("No Discrete GPU Detected")
        .set_description(description)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();

    matches!(choice, rfd::MessageDialogResult::Yes)
}

fn enforce_discrete_gpu_policy_or_exit() {
    let probe = probe_gpu_policy();

    if probe.has_discrete_gpu {
        std::env::set_var("WGPU_POWER_PREF", "high");

        if let Some(adapter_name) = probe.selected_gpu_name {
            std::env::set_var("WGPU_ADAPTER_NAME", adapter_name);
        }

        tracing::info!("Discrete GPU detected; forcing high-performance GPU preference");
        return;
    }

    tracing::warn!("No discrete GPU detected; prompting user for continue/exit decision");
    if !prompt_continue_without_discrete_gpu() {
        tracing::error!("Startup aborted by user because no discrete GPU is available");
        std::process::exit(1);
    }

    tracing::warn!("User chose to continue without a discrete GPU");
}

fn maybe_prompt_project_file_association() {
    let manager = match FileAssociationManager::system() {
        Ok(manager) => manager,
        Err(AssociationError::ToolMissing(tool)) => {
            tracing::warn!(
                "File association tooling is missing on this machine: {}",
                tool
            );

            let message = if cfg!(target_os = "macos") && tool == "duti" {
                "Pulsar could not check or set project file associations because 'duti' is not installed.\n\nInstall it with:\n  brew install duti\n\nThen relaunch Pulsar to enable one-click association for Pulsar.toml."
            } else {
                "Pulsar could not check or set project file associations because a required tool is missing.\n\nInstall the required association tool and relaunch Pulsar."
            };

            let _ = rfd::MessageDialog::new()
                .set_title("Pulsar Project Association")
                .set_description(message)
                .set_level(rfd::MessageLevel::Warning)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return;
        }
        Err(err) => {
            tracing::debug!("Skipping file association check: {}", err);
            return;
        }
    };

    let request = match build_project_association_request() {
        Some(req) => req,
        None => {
            tracing::debug!("No file association request is available for this platform");
            #[cfg(target_os = "macos")]
            {
                let _ = rfd::MessageDialog::new()
                    .set_title("Pulsar Project Association")
                    .set_description(
                        "Pulsar could not determine a valid TOML UTI on this macOS installation, so association was skipped.",
                    )
                    .set_level(rfd::MessageLevel::Warning)
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
            }
            return;
        }
    };

    let already_associated = manager
        .query(association_query_target())
        .ok()
        .flatten()
        .map(|record| record.handler_id == request.handler_id)
        .unwrap_or(false);

    if already_associated {
        tracing::debug!(
            "Project descriptor association already points to this engine handler ({})",
            request.handler_id
        );
        return;
    }

    let should_associate = rfd::MessageDialog::new()
        .set_title("Associate Pulsar Project Files")
        .set_description(format!(
            "Pulsar can associate project descriptor files ({}) with this running engine build (v{}).\n\nOn macOS this is applied via the TOML UTI mapping.\n\nAssociate now?",
            PROJECT_ASSOC_EXTENSION,
            consts::ENGINE_VERSION,
        ))
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();

    if !matches!(should_associate, rfd::MessageDialogResult::Yes) {
        tracing::debug!("User declined Pulsar project file association prompt");
        return;
    }

    match manager.set(request) {
        Ok(()) => {
            tracing::info!("Project descriptor file association updated successfully");
            let _ = rfd::MessageDialog::new()
                .set_title("Pulsar Project Association")
                .set_description("Pulsar project descriptor association was updated for this engine build.")
                .set_level(rfd::MessageLevel::Info)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
        Err(err) => {
            tracing::warn!("Failed to update project file association: {}", err);
            let _ = rfd::MessageDialog::new()
                .set_title("Pulsar Project Association")
                .set_description(format!(
                    "Pulsar could not update file associations automatically.\n\n{}",
                    err
                ))
                .set_level(rfd::MessageLevel::Warning)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
    }
}

fn build_project_association_request() -> Option<AssociationRequest> {
    let exe_path = std::env::current_exe().ok()?;

    #[cfg(target_os = "windows")]
    {
        let handler_id = format!(
            "dev.pulsar.engine.{}",
            consts::ENGINE_VERSION.replace('.', "_")
        );
        let command = format!("\"{}\" \"%1\"", exe_path.display());
        return Some(
            AssociationRequest::new(PROJECT_ASSOC_EXTENSION, handler_id)
                .with_mime_type(PROJECT_ASSOC_MIME)
                .with_command(command),
        );
    }

    #[cfg(target_os = "macos")]
    {
        let uti = detect_macos_toml_uti()?;
        return Some(
            AssociationRequest::new(uti, MACOS_BUNDLE_ID)
                .with_mime_type(PROJECT_ASSOC_MIME),
        );
    }

    #[cfg(target_os = "linux")]
    {
        let handler_id = ensure_linux_desktop_entry(&exe_path)?;
        return Some(
            AssociationRequest::new(PROJECT_ASSOC_EXTENSION, handler_id)
                .with_mime_type(PROJECT_ASSOC_MIME),
        );
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn association_query_target() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        return MACOS_ASSOC_QUERY_EXTENSION;
    }

    #[cfg(not(target_os = "macos"))]
    {
        PROJECT_ASSOC_EXTENSION
    }
}

#[cfg(target_os = "macos")]
fn detect_macos_toml_uti() -> Option<String> {
    let probe_path = std::env::temp_dir().join(format!(
        "pulsar-assoc-probe-{}-{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos(),
        MACOS_ASSOC_QUERY_EXTENSION
    ));

    if std::fs::write(&probe_path, b"").is_err() {
        return None;
    }

    let output = std::process::Command::new("mdls")
        .args([
            "-raw",
            "-name",
            "kMDItemContentType",
            &probe_path.to_string_lossy(),
        ])
        .output()
        .ok()?;

    let _ = std::fs::remove_file(&probe_path);

    if !output.status.success() {
        return None;
    }

    let uti = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('"')
        .to_string();

    if uti.is_empty() || uti == "(null)" || uti.contains('/') || uti.ends_with(".app") {
        return None;
    }

    Some(uti)
}

#[cfg(target_os = "linux")]
fn ensure_linux_desktop_entry(exe_path: &std::path::Path) -> Option<String> {
    let base_dirs = directories::BaseDirs::new()?;
    let desktop_dir = base_dirs.data_dir().join("applications");
    std::fs::create_dir_all(&desktop_dir).ok()?;

    let desktop_file_name = format!(
        "pulsar-engine-{}.desktop",
        consts::ENGINE_VERSION.replace('.', "-")
    );
    let desktop_path = desktop_dir.join(&desktop_file_name);

    let escaped_exe = exe_path.display().to_string().replace('"', "\\\"");
    let desktop_content = format!(
        "[Desktop Entry]\nType=Application\nName=Pulsar Engine\nExec=\"{}\" %f\nTerminal=false\nMimeType={};\nCategories=Development;IDE;\n",
        escaped_exe, PROJECT_ASSOC_MIME
    );

    let current = std::fs::read_to_string(&desktop_path).unwrap_or_default();
    if current != desktop_content {
        std::fs::write(&desktop_path, desktop_content).ok()?;
    }

    let _ = std::process::Command::new("update-desktop-database")
        .arg(&desktop_dir)
        .output();

    Some(desktop_file_name)
}

/// Main entry point for the Pulsar Engine binary.
///
/// Uses dependency graph-based initialization for explicit ordering and validation.
fn main() {
    enforce_discrete_gpu_policy_or_exit();

    macos_permissions::ensure_accessibility_permission_blocking();

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
                maybe_prompt_project_file_association();
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
        cx.activate(true);
        ui::init(cx);
        ui::themes::init(cx);
        ui_core::init(cx);

        {
            use window_manager::WindowManager;
            cx.set_global(WindowManager::new());
        }

        let mut launch = engine_context.launch.write();

        if let Some(path) = launch.uri_project_path.take() {
            tracing::info!("Opening project splash from URI: {}", path.display());
            open_via_loading_screen(path, cx);
        } else {
            tracing::info!("Opening main entry window");
            let ec = engine_context.clone();
            let opts = make_window_options(
                Some("Pulsar Engine"),
                gpui::point(gpui::px(100.0), gpui::px(100.0)),
                gpui::size(gpui::px(1100.0), gpui::px(700.0)),
                Some(gpui::Size { width: gpui::px(800.), height: gpui::px(500.) }),
            );
            match engine_context.create_window(
                WindowRequest::Entry,
                opts,
                move |window, cx| {
                    let project_cb: std::sync::Arc<dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(|pathbuf, cx| open_via_loading_screen(pathbuf, cx));

                    let git_cb: std::sync::Arc<dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(|pathbuf, cx| {
                            ui_common::open_window::open_pulsar_window::<ui_git_manager::GitManager>(pathbuf, cx);
                        });

                    let settings_cb: std::sync::Arc<dyn Fn(&mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(|cx| {
                            ui_common::open_window::open_pulsar_window::<ui_settings::SettingsWindow>((), cx);
                        });

                    let fab_cb: std::sync::Arc<dyn Fn(&mut gpui::App) + Send + Sync> =
                        std::sync::Arc::new(|cx| {
                            ui_common::open_window::open_pulsar_window::<ui_fab_search::FabSearchWindow>((), cx);
                        });

                    ui_entry::create_entry_component(window, cx, &ec, 0, project_cb, git_cb, settings_cb, fab_cb)
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
/// This is the **single canonical path** for opening an editor window — both the
/// URI-launch flow and the entry-screen project-open callback go through here so
/// that the editor window is always created with the same options and builder.
fn open_via_loading_screen(path: std::path::PathBuf, cx: &mut gpui::App) {
    let on_complete: std::sync::Arc<dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync> =
        std::sync::Arc::new(|path, cx| {
            let opts = make_window_options(
                Some("Pulsar Engine"),
                gpui::point(gpui::px(50.0), gpui::px(50.0)),
                gpui::size(gpui::px(1600.0), gpui::px(900.0)),
                Some(gpui::Size {
                    width: gpui::px(800.),
                    height: gpui::px(600.),
                }),
            );
            let _ = cx.open_window(opts, move |window, cx| {
                let app =
                    cx.new(|cx| ui_core::PulsarApp::new_with_project(path.clone(), window, cx));
                let root = cx.new(|cx| ui_core::PulsarRoot::new("Pulsar Engine", app, window, cx));
                cx.new(|cx| ui::Root::new(root.into(), window, cx))
            });
        });
    ui_common::open_window::open_pulsar_window::<ui_loading_screen::LoadingScreen>(
        (path, on_complete),
        cx,
    );
}

/// Build common `WindowOptions` to reduce boilerplate.
fn make_window_options(
    _title: Option<&'static str>,
    origin: gpui::Point<gpui::Pixels>,
    win_size: gpui::Size<gpui::Pixels>,
    min_size: Option<gpui::Size<gpui::Pixels>>,
) -> gpui::WindowOptions {
    // Embed the Pulsar icon at compile time so it is always available at runtime,
    // even when running outside an app bundle (no .icns / no PE resource needed).
    static ICON_PNG: &[u8] = include_bytes!("../../../assets/images/logo_sqrkl_mac.png");
    let app_icon = gpui::WindowIcon::from_png_bytes(ICON_PNG)
        .map_err(|e| tracing::warn!("Failed to decode app icon: {e}"))
        .ok();

    gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
            origin, win_size,
        ))),
        titlebar: None,
        kind: gpui::WindowKind::Normal,
        is_resizable: true,
        window_decorations: Some(gpui::WindowDecorations::Client),
        window_min_size: min_size,
        app_icon,
        window_background: gpui::WindowBackgroundAppearance::Opaque,
        // always_transparent: false,
        ..Default::default()
    }
}
