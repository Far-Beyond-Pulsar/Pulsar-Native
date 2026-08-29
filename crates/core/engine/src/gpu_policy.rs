//! GPU policy detection and enforcement
//!
//! This module handles discrete GPU detection and policy enforcement for optimal
//! rendering performance. It probes available GPUs, prompts the user if no discrete
//! GPU is available, and configures WGPU environment variables accordingly.

use wgpu::{Backends, DeviceType, Instance, InstanceDescriptor};

#[derive(Debug, Clone)]
struct GpuPolicyProbe {
    has_discrete_gpu: bool,
    selected_gpu_name: Option<String>,
}

/// Detect if running under WSL (Windows Subsystem for Linux).
fn is_wsl() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/version")
            .map(|v| v.to_lowercase().contains("microsoft"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Probe the system for available GPUs and detect if a discrete GPU is present
fn probe_gpu_policy() -> GpuPolicyProbe {
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::all(),
        flags: wgpu::InstanceFlags::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
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

/// Prompt the user whether to continue without a discrete GPU.
///
/// On headless systems (WSL without WSLg, SSH sessions), the dialog cannot
/// display — fall back to a stderr warning and continue automatically.
fn prompt_continue_without_discrete_gpu() -> bool {
    let description = "No discrete GPU was detected.\n\nPulsar is configured to prefer dGPU for best performance.\n\nContinue anyway using the available GPU?";

    // On headless Linux/WSL without a display server, rfd cannot show a dialog.
    // Check for DISPLAY/WAYLAND_DISPLAY before attempting the GUI prompt.
    #[cfg(target_os = "linux")]
    {
        let has_display = std::env::var("DISPLAY").is_ok()
            || std::env::var("WAYLAND_DISPLAY").is_ok()
            || std::env::var("WLR_RDP_BACKENDS").is_ok();

        if !has_display {
            eprintln!("[gpu_policy] No display server detected (headless/WSL). Continuing with available GPU.");
            return true;
        }
    }

    let choice = rfd::MessageDialog::new()
        .set_title("No Discrete GPU Detected")
        .set_description(description)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();

    matches!(choice, rfd::MessageDialogResult::Yes)
}

/// Enforce discrete GPU policy or exit if user declines to continue
///
/// This function:
/// 1. Probes for discrete GPUs
/// 2. If found, sets WGPU environment variables to prefer high-performance GPU
/// 3. If not found, prompts user whether to continue
/// 4. Exits the application if user declines
pub fn enforce_discrete_gpu_policy_or_exit() {
    // Apple Silicon (M-series) has an integrated GPU that's plenty capable,
    // and the WGPU driver handles power management natively — skip the check.
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        tracing::info!("Apple Silicon detected; skipping discrete GPU policy check");
        return;
    }

    // WSL provides GPU passthrough or virtualized rendering — discrete GPU
    // detection is irrelevant and the prompt dialog cannot display without WSLg.
    if is_wsl() {
        tracing::info!("WSL detected; skipping discrete GPU policy check");
        return;
    }

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
