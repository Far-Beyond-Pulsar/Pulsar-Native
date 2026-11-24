//! Startup after the init graph: the GPUI application and its device
//! options, the hardware summary, the UI crates' initialization and opening
//! the project the process was launched for.

/// The GPUI application configured from the renderer settings, with the
/// wgpu backends it was asked to use (the loading screen reports them).
pub(crate) fn build_gpui_application() -> (gpui::Application, wgpu::Backends) {
    let renderer_settings = engine_state::settings::global_config();
    let renderer_string = |key: &str, default: &str| {
        renderer_settings
            .get(engine_state::settings::NS_EDITOR, "renderer", key)
            .ok()
            .and_then(|value| value.as_str().ok().map(str::to_owned))
            .unwrap_or_else(|| default.to_owned())
    };
    let renderer_backends = engine_state::settings::renderer_backends();
    let power_preference = match renderer_string("gpu_preference", "high_performance").as_str() {
        "low_power" => wgpu::PowerPreference::LowPower,
        "auto" => wgpu::PowerPreference::None,
        _ => wgpu::PowerPreference::HighPerformance,
    };
    let max_frame_latency = renderer_settings
        .get(
            engine_state::settings::NS_EDITOR,
            "renderer",
            "max_frame_latency",
        )
        .ok()
        .and_then(|value| value.as_int().ok())
        .unwrap_or(2)
        .clamp(1, 4) as u32;

    let gpui_app = gpui::Application::with_wgpu_options(gpui::WgpuOptions {
        additional_features: wgpu::Features::VERTEX_WRITABLE_STORAGE,
        // TEXTURE_COMPRESSION_BC: cooked textures are stored as BC7 where the device can sample it
        optional_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
        desired_maximum_frame_latency: max_frame_latency,
        backends: renderer_backends,
        power_preference,
        // Ray queries are the engine's to ask for, in the device factory below.
        hardware_ray_queries: false,
        preferred_backends: wgpu::Backends::empty(),
        // The window's device is the engine's (`RenderRuntimes::offer_device`): request it with
        // Helio's features and limits on top of the window's own.
        device_factory: Some(std::sync::Arc::new(|adapter, descriptor| {
            engine_backend::services::render_runtimes::request_engine_device(adapter, descriptor)
                .map_err(anyhow::Error::from)
        })),
    })
    .with_assets(crate::Assets);

    (gpui_app, renderer_backends)
}

/// The other graphics backends this machine offers, as (label, preference)
/// pairs, so the loading screen can suggest them when Vulkan is in use.
fn backend_alternatives(renderer_backends: wgpu::Backends) -> Vec<(String, String)> {
    if renderer_backends != wgpu::Backends::VULKAN {
        return Vec::new();
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        flags: wgpu::InstanceFlags::default(),
        backend_options: wgpu::BackendOptions::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    });
    let adapters = futures::executor::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let mut seen = std::collections::HashSet::new();
    adapters
        .into_iter()
        .filter_map(|adapter| {
            let (label, preference) = match adapter.get_info().backend {
                wgpu::Backend::Dx12 => ("Direct3D 12", "dx12"),
                wgpu::Backend::Metal => ("Metal", "metal"),
                wgpu::Backend::Gl => ("OpenGL", "gl"),
                _ => return None,
            };
            seen.insert(preference)
                .then(|| (label.to_owned(), preference.to_owned()))
        })
        .collect()
}

/// The hardware summary the loading screen shows.
///
/// Reuses the system snapshot already used by Mission Control so startup
/// compatibility checks and the diagnostics panel report the same device.
pub(crate) fn gather_hardware_profile(
    renderer_backends: wgpu::Backends,
) -> ui_loading_screen::HardwareProfile {
    let system_info = ui_log_viewer::SystemInfo::gather();
    let backend_alternatives = backend_alternatives(renderer_backends);
    ui_loading_screen::HardwareProfile {
        backend: if renderer_backends == wgpu::Backends::VULKAN {
            "Vulkan".to_owned()
        } else {
            format!("{renderer_backends:?}")
        },
        backend_alternatives,
        manufacturer: system_info.gpu_vendor,
        gpu_brand: system_info.gpu_name,
        driver: system_info.gpu_driver_version,
        os: format!("{} {}", system_info.os_name, system_info.os_version),
    }
}

/// Install the UI globals and initialize the UI crates, in dependency order.
pub(crate) fn init_ui_stack(cx: &mut gpui::App) {
    // Installed before any init: window crates self-register into
    // WindowRegistry during their own init (e.g. SettingsWindow::init).
    {
        use window_manager::{WindowManager, WindowRegistry};
        cx.set_global(WindowManager::new());
        cx.set_global(WindowRegistry::new());
    }

    let t = std::time::Instant::now();
    ui::init(cx);
    tracing::info!("[GPUI startup] ui::init {}ms", t.elapsed().as_millis());

    let t = std::time::Instant::now();
    ui::themes::init(cx);
    apply_appearance_settings(cx);
    tracing::info!(
        "[GPUI startup] ui::themes::init {}ms",
        t.elapsed().as_millis()
    );

    let t = std::time::Instant::now();
    ui_core::init(cx);
    tracing::info!("[GPUI startup] ui_core::init {}ms", t.elapsed().as_millis());

    // The MCP server host: the UI-thread bridge, the existing AI tools as MCP tools, and the
    // settings watcher that starts the server when Editor Settings turn it on.
    ui_mcp::init(cx);

    // Runs every inventory::submit! registrant from all linked crates automatically.
    let t = std::time::Instant::now();
    window_manager::register_all_windows(cx);
    tracing::info!(
        "[GPUI startup] register_all_windows {}ms",
        t.elapsed().as_millis()
    );
}

/// Apply the editor's appearance settings (theme and color-blind mode, font
/// size, corner radius, scrollbars) to the global theme.
fn apply_appearance_settings(cx: &mut gpui::App) {
    ui_common::appearance::apply(cx);
}

/// Open the project the process was launched for: the one from a `pulsar://`
/// URI, else `--project-path`; quit when neither names a usable directory.
pub(crate) fn open_startup_project(
    uri_path: Option<std::path::PathBuf>,
    project_path: Option<std::path::PathBuf>,
    hardware_profile: &ui_loading_screen::HardwareProfile,
    cx: &mut gpui::App,
) {
    if let Some(path) = uri_path {
        tracing::info!("Opening project splash from URI: {}", path.display());
        open_via_loading_screen(path, hardware_profile, cx);
        return;
    }
    match project_path {
        Some(path) if engine_fs::virtual_fs::is_dir(&path) => {
            tracing::info!("Opening project from --project-path: {}", path.display());
            open_via_loading_screen(path, hardware_profile, cx);
        }
        Some(path) => {
            if engine_fs::virtual_fs::exists(&path).unwrap_or(false) {
                tracing::error!("Project path is not a directory: {}", path.display());
            } else {
                tracing::error!("Project path does not exist: {}", path.display());
            }
            cx.quit();
        }
        None => {
            tracing::warn!("No project specified. Pass one with --project-path <PROJECT_PATH> or launch via Pulsar Hub.");
            cx.quit();
        }
    }
}

// Open a project through the loading-screen splash, then transition to the editor.
///
/// Single canonical path for opening an editor window — URI-launch and entry-screen
/// project-open both go through here.
fn open_via_loading_screen(
    path: std::path::PathBuf,
    hardware_profile: &ui_loading_screen::HardwareProfile,
    cx: &mut gpui::App,
) {
    use ui_common::PulsarWindowExt as _;
    let on_complete: std::sync::Arc<dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync> =
        std::sync::Arc::new(|path, cx| {
            ui_core::PulsarRoot::open(path, cx);
        });
    ui_loading_screen::open_before_loading(
        path,
        on_complete,
        hardware_profile.clone(),
        env!("CARGO_PKG_VERSION"),
        cx,
    );
}
