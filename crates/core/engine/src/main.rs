//! Pulsar Engine binary: process setup, the startup task graph, and the GPUI
//! event loop.
//!
//! Startup runs as an `InitGraph` of tasks that declare their dependencies
//! (see `run_init_graph`). The graph is checked for cycles and missing
//! dependencies, then executed in topological order; each task is profiled as
//! `Engine::Init::{TaskName}`.
//!
//! Tasks and what each waits for:
//!
//! - Logging (nothing), then App Data, then Settings.
//! - Async Runtime (Logging), then Engine Backend.
//! - Engine Context (nothing), then Dev/Source Detection, Set Global Context
//!   and Discord Rich Presence, in that chain.
//! - URI Scheme Registration (Runtime).
//!
//! Windows are managed by GPUI. After the graph has run, `startup` builds the
//! GPUI application and opens the first window.

#![warn(clippy::undocumented_unsafe_blocks)]

// `#![windows_subsystem = "windows"]` is deliberately not set: the binary keeps
// the console subsystem, so the stdout log layer (`logging`) is visible when
// the editor is launched from a terminal.

// --- Global Allocator Setup ---
// Only one #[global_allocator] can be registered, so the dhat heap profiler
// (feature = "dhat-heap") and the normal in-editor tracking allocator are
// mutually exclusive.
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static GLOBAL_ALLOCATOR: dhat::Alloc = dhat::Alloc;

#[cfg(not(feature = "dhat-heap"))]
use ui_log_viewer::TrackingAllocator;

// The plugins the editor links: render_bps only. Every other installed plugin is a
// library of its own (tools/plugin_builder) that the plugin loader loads from
// plugins/editor at startup; the project the editor opens enables some of them
// (`pulsar_plugins`).
use linked_plugins as _;
// The shared UI framework and renderer: this binary and every plugin library use these
// copies (see .agents/PLUGIN_SYSTEM.md, "Shared libraries").
use helio_dylib as _;
use profiling_dylib as _;
use ui_dylib as _;
use reflection_dylib as _;
use scenedb_dylib as _;
use wgpui_dylib as _;
// The content packer the build's pack step uses (`pulsar_content::ContentPacker`).
use pulsar_package as _;

// The optional editor tools this editor ships: linking one installs its
// registrations (`ui_common::editor_tools`, the chat's providers).
use agent_provider_anthropic as _;
use agent_provider_aws_bedrock as _;
use agent_provider_demo_random as _;
use agent_provider_docker_model_runner as _;
use agent_provider_gemini as _;
use agent_provider_github_copilot as _;
use agent_provider_openai as _;
use agent_provider_opencode as _;
use agent_provider_vertex_ai as _;
use ui_fab_search as _;
use ui_flamegraph as _;
use ui_git_manager as _;

#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

// Re-export render from backend where it actually lives
pub use engine_backend::subsystems::render;
// Re-export graph from ui crate (canonical location)
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
mod dev_bootstrap;
pub mod discord; // Discord Rich Presence integration
#[cfg(all(windows, target_arch = "aarch64"))]
mod dxc; // Install and select DXC: Windows on ARM has no static DXC and FXC rejects our shaders
pub mod gpu_policy; // GPU detection and policy enforcement
pub mod init; // Initialization dependency graph
pub mod logging; // Logging setup and configuration
pub mod macos_permissions;
#[cfg(all(windows, target_arch = "aarch64"))]
mod prompts; // Startup prompts, shown by the pulsar-dialog helper (only DXC install prompts them)
pub mod runtime; // Async runtime setup and management
pub mod settings; // Engine settings loading and saving
mod startup;
mod steps;
pub mod uri; // URI scheme handling

// --- Engine context re-exports ---
pub use engine_state::{
    EngineContext,
    // sender/receiver removed as messaging is no longer used
    LaunchContext,
    WindowRequest,
};

use init::{init_task, task_ids::*, InitContext, InitGraph};

/// Give the engine's render graphs a stable cache path beside Cargo's build
/// artifacts. An explicit path is useful for hosts that coordinate several
/// processes (the play-mode runner does this) and always takes precedence.
///
/// The Helio graph builder (the `crates/renderer/helio` submodule, which cannot
/// depend on `engine_state`) reads this variable directly, as do child
/// processes such as the play-mode runner. That is why the hand-off is still an
/// environment variable: configure it before initialization can start any
/// renderer work. This is the only place the variable is read or written on the
/// engine side.
fn configure_pipeline_cache_path() {
    if std::env::var_os("PULSAR_PIPELINE_CACHE").is_some() {
        return;
    }

    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(&path))
                    .unwrap_or(path)
            }
        })
        .or_else(|| {
            std::env::current_exe().ok().and_then(|executable| {
                executable
                    .ancestors()
                    .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "target"))
                    .map(std::path::Path::to_path_buf)
            })
        });

    if let Some(target_dir) = target_dir {
        std::env::set_var(
            "PULSAR_PIPELINE_CACHE",
            target_dir.join("pulsar").join("pipeline_cache.bin"),
        );
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static NvOptimusEnablement: u32 = 0x0000_0001;

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static AmdPowerXpressRequestHighPerformance: u32 = 0x0000_0001;

/// Main entry point for the Pulsar Engine binary.
///
/// Uses dependency graph-based initialization for explicit ordering and validation.
fn main() {
    if dev_bootstrap::run_if_requested() {
        return;
    }

    // Held for the entire function body so it dumps `dhat-heap.json` (via
    // Drop) on clean process exit — e.g. quitting the editor normally after
    // reproducing a leak. Only active with `--features dhat-heap`.
    #[cfg(feature = "dhat-heap")]
    let _dhat_profiler = dhat::Profiler::new_heap();

    prepare_process();

    // Name the main thread FIRST
    profiling::set_thread_name("Main Thread");
    // The UI thread is listed in the Tasks window's Threads view, with the
    // task it is running (tab creation, renderer setup) when there is one.
    let _main_thread = editor_task_queue::register_current_thread(
        editor_task_queue::ThreadSpec::new("Main Thread", editor_task_queue::ThreadRole::Main)
            .owner("engine"),
    );

    install_flamegraph_scope_hook();

    // Profiling is intentionally NOT enabled here. Instrumentation
    // (profile_scope!) submits events into an unbounded channel the moment
    // profiling is enabled, and nothing drains that channel unless the
    // Flamegraph panel is actively recording — leaving it on for the whole
    // process lifetime leaks memory at a steady rate for as long as the
    // editor window is open. The Flamegraph panel's InstrumentationCollector
    // enables/disables profiling itself, scoped to its recording session.

    // Parse arguments first (needed for init context)
    let _ = dotenv::dotenv();
    configure_pipeline_cache_path();
    let parsed = args::parse_args();
    let project_path = parsed.project_path.clone();

    // Create initialization context
    let mut init_ctx = InitContext::new(parsed.clone());
    run_init_graph(&mut init_ctx);

    // GPU policy check — runs after logging is initialized so warnings/errors are visible.
    gpu_policy::enforce_discrete_gpu_policy_or_exit();

    // Extract initialized components
    let engine_context = init_ctx.engine_context.unwrap_or_else(|| {
        tracing::error!("Failed to initialize engine context");
        std::process::exit(1);
    });

    // Run the main event loop via GPUI's `App::run` API.
    profiling::profile_scope!("Engine::EventLoop");

    // create and run GPUI application
    let (gpui_app, renderer_backends) = startup::build_gpui_application();
    let hardware_profile = startup::gather_hardware_profile(renderer_backends);

    gpui_app.run(move |cx: &mut gpui::App| {
        profiling_v1::scope!("pulsar: gpui app run");
        let t_gpui = std::time::Instant::now();
        tracing::info!("[GPUI startup] begin");

        cx.activate(true);

        // The UI thread only builds and updates views. The framework names
        // each piece of work it runs here (event, view render, listener,
        // foreground future); any that runs past the budget is recorded as a
        // failed "Slow UI work" task, and the responsiveness watcher names the
        // piece it caught when the UI stops (.agents/EDITOR_TASKS.md).
        gpui::ui_work::install(
            std::time::Duration::from_millis(50),
            editor_task_queue::record_slow_ui_work,
        );
        editor_task_queue::set_ui_activity_source(gpui::ui_work::current);

        startup::init_ui_stack(cx);

        let uri_path = engine_context
            .store
            .get_or_init::<engine_state::LaunchContext>()
            .update(|l| l.uri_project_path.take());

        tracing::info!(
            "[GPUI startup] pre-window total {}ms",
            t_gpui.elapsed().as_millis()
        );

        startup::open_startup_project(uri_path, project_path, &hardware_profile, cx);

        // The UI thread's beat: a timer on its own executor, so it only fires
        // when the thread is free. When it stops, the task queue records a
        // failed "UI thread unresponsive" task naming what held the thread.
        cx.spawn(async move |cx| loop {
            editor_task_queue::ui_beat();
            cx.background_executor()
                .timer(editor_task_queue::UI_BEAT_EVERY)
                .await;
        })
        .detach();
    });
}

/// Process-wide setup that must happen before anything else in `main`: TLS
/// provider, the dialog helper, the DXC requirement and the startup hint.
fn prepare_process() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // The dialog helper rides inside this executable; put it beside us if it is
    // not there, before the first prompt that might need it. Failing is fine:
    // dialogs then fall back to the platform's own boxes.
    match pulsar_dialog_embed::ensure_helper() {
        Ok(placement) => tracing::debug!(?placement, "dialog helper"),
        Err(error) => tracing::warn!(%error, "could not place the dialog helper"),
    }

    // Before any wgpu instance: never run on FXC.
    #[cfg(all(windows, target_arch = "aarch64"))]
    dxc::require_or_exit();

    macos_permissions::ensure_accessibility_permission_blocking();

    println!(
        "If Pulsar Engine exits immediately, launch it from Pulsar Hub (https://github.com/Far-Beyond-Pulsar/Pulsar-Hub) or pass a project path manually with `--project-path <PROJECT_PATH>`."
    );
}

/// Route wgpui's frame-stage scopes (draw/layout/prepaint/paint/present,
/// swapchain acquire, surface-registry and submit-lock waits, ...) into the
/// flamegraph's profiler. wgpui links the crates.io `profiling` crate, a
/// different instance with its own global state, so without this bridge the
/// entire UI thread is invisible to the flamegraph.
fn install_flamegraph_scope_hook() {
    gpui::render_stats::set_scope_hook(gpui::render_stats::ScopeHook {
        enabled: profiling::is_profiling_enabled,
        begin: |name| Box::new(profiling::ProfileScope::new_static(name)),
        begin_owned: |name| Box::new(profiling::ProfileScope::new(name)),
        record_elapsed: |name, elapsed| profiling::record_elapsed(name, elapsed),
    });
}

/// Declare the startup tasks and their dependencies, then run them in order.
/// A failed task ends the process.
fn run_init_graph(init_ctx: &mut InitContext) {
    let mut graph = InitGraph::new();

    // Logging (no dependencies)
    init_task!(graph, LOGGING, "Logging", [], steps::logging::run);

    // App data (depends on logging)
    init_task!(graph, APPDATA, "App Data", [LOGGING], steps::appdata::run);

    // Settings (depends on app data)
    init_task!(graph, SETTINGS, "Settings", [APPDATA], steps::settings::run);

    // Runtime (depends on logging)
    init_task!(
        graph,
        RUNTIME,
        "Async Runtime",
        [LOGGING],
        steps::runtime::run
    );

    // Backend (depends on runtime)
    init_task!(
        graph,
        BACKEND,
        "Engine Backend",
        [RUNTIME],
        steps::backend::run
    );

    // Engine Context (no dependencies)
    init_task!(
        graph,
        ENGINE_CONTEXT,
        "Engine Context",
        [],
        steps::engine_context::run
    );

    // Dev Detection (depends on engine context, before set_global)
    init_task!(
        graph,
        DEV_DETECT,
        "Dev/Source Detection",
        [ENGINE_CONTEXT],
        steps::dev_detect::run
    );

    // Set Global (depends on dev detection)
    init_task!(
        graph,
        SET_GLOBAL,
        "Set Global Context",
        [DEV_DETECT],
        steps::set_global::run
    );

    // Discord (depends on set_global)
    init_task!(
        graph,
        DISCORD,
        "Discord Rich Presence",
        [SET_GLOBAL],
        steps::discord::run
    );

    // URI Registration (depends on runtime)
    init_task!(
        graph,
        URI_REGISTRATION,
        "URI Scheme Registration",
        [RUNTIME],
        steps::uri_registration::run
    );

    // Execute the initialization graph
    {
        profiling_v1::scope!("pulsar: init graph execute");
        if let Err(e) = graph.execute(init_ctx) {
            tracing::error!("Engine initialization failed: {}", e);
            std::process::exit(1);
        }
    }
}
