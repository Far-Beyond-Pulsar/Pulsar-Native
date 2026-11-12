//! GPU policy detection and enforcement
//!
//! This module handles discrete GPU detection and policy enforcement for optimal
//! rendering performance. It probes available GPUs, prompts the user if no discrete
//! GPU is available, and configures WGPU environment variables accordingly.

use pulsar_dialog_protocol::{client, ButtonKind, DialogButton, DialogRequest, DialogResponse, Severity};
use wgpu::{Backends, DeviceType, Instance, InstanceDescriptor};

#[derive(Debug, Clone)]
struct GpuPolicyProbe {
    has_discrete_gpu: bool,
    selected_gpu_name: Option<String>,
    /// Every adapter found, as `"name (integrated GPU)"`, for the prompt.
    adapters: Vec<String>,
}

/// The ids of the prompt's buttons.
const CONTINUE: &str = "continue";
const QUIT: &str = "quit";

/// Whether an adapter counts as a discrete GPU for the policy.
///
/// Drivers say `DiscreteGpu` for a card on a PCIe slot. NVIDIA's RTX Spark
/// systems are a full Blackwell GPU sharing the CPU's memory, so their driver
/// reports `IntegratedGpu` (and a duplicate `Other` entry on some APIs), yet
/// they are discrete GPUs and the machine's high-performance adapter. Any
/// adapter whose name contains "RTX Spark" counts.
fn counts_as_discrete(device_type: DeviceType, name: &str) -> bool {
    device_type == DeviceType::DiscreteGpu || name.to_lowercase().contains("rtx spark")
}

fn device_kind(device_type: DeviceType) -> &'static str {
    match device_type {
        DeviceType::DiscreteGpu => "discrete GPU",
        DeviceType::IntegratedGpu => "integrated GPU",
        DeviceType::VirtualGpu => "virtual GPU",
        DeviceType::Cpu => "software renderer",
        DeviceType::Other => "other",
    }
}

// std::fs allowed: reads the OS's /proc/version to detect WSL, not a project file
#[allow(clippy::disallowed_methods)]
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
    let mut found = Vec::new();

    for adapter in adapters {
        let info = adapter.get_info();
        let described = format!("{} ({})", info.name, device_kind(info.device_type));
        // One line per adapter: the same card shows up once per graphics API.
        if !found.contains(&described) {
            found.push(described);
        }
        if counts_as_discrete(info.device_type, &info.name) {
            has_discrete_gpu = true;
            if selected_gpu_name.is_none() {
                selected_gpu_name = Some(info.name);
            }
        }
    }

    GpuPolicyProbe {
        has_discrete_gpu,
        selected_gpu_name,
        adapters: found,
    }
}

/// The question: continue on the GPU that is there, or quit. "Don't ask
/// again" sets the existing Automatic GPU preference.
fn prompt_request(adapters: &[String]) -> DialogRequest {
    let mut request = DialogRequest::new(
        "No Discrete GPU Detected",
        "Pulsar is set to prefer a dedicated GPU for the best performance, and none was \
         detected.\n\nContinue anyway, using the GPU that is available?",
    )
    .severity(Severity::Warning)
    .button(DialogButton::new(CONTINUE, "Continue").kind(ButtonKind::Primary).default())
    .button(DialogButton::new(QUIT, "Quit").cancel())
    .checkbox("Don't ask again (set the GPU preference to Automatic)", false);
    if !adapters.is_empty() {
        request = request.detail(format!("Available: {}", adapters.join(", ")));
    }
    request
}

/// What an answer means: `(continue, remember)`. Closing the dialog is
/// declining; no dialog at all (no display, no helper) is carrying on, as
/// the prompt always did on a headless machine.
fn decide(answer: &DialogResponse) -> (bool, bool) {
    use pulsar_dialog_protocol::Outcome;
    match answer.outcome {
        Outcome::Unavailable => (true, false),
        Outcome::Dismissed => (false, false),
        Outcome::Answered if answer.is(CONTINUE) => (true, answer.checkbox == Some(true)),
        Outcome::Answered => (false, false),
    }
}

/// Persist "Automatic" as the GPU preference, so the prompt stops appearing.
fn remember_automatic_preference() {
    use engine_state::settings::{ConfigValue, GlobalSettings};
    match GlobalSettings::new().set_and_save(
        "renderer",
        "gpu_preference",
        ConfigValue::String("auto".to_owned()),
    ) {
        Ok(()) => tracing::info!("GPU preference set to Automatic at the user's request"),
        Err(error) => tracing::warn!(%error, "Could not save the GPU preference"),
    }
}

/// Prompt the user whether to continue without a discrete GPU.
///
/// The question is shown by the `pulsar-dialog` helper (its own process, so a
/// broken graphics setup here cannot take the prompt down with it), in the
/// engine's look. If the helper cannot run, the platform's own message box is
/// used; with no display at all (WSL without WSLg, SSH sessions) the answer is
/// "continue" with a note on stderr.
fn prompt_continue_without_discrete_gpu(adapters: &[String]) -> bool {
    let answer = client::show(&prompt_request(adapters));
    if client::is_unavailable(&answer) {
        eprintln!(
            "[gpu_policy] The dialog could not be shown (no display or helper). \
             Continuing with the available GPU."
        );
    }
    let (continue_anyway, remember) = decide(&answer);
    if continue_anyway && remember {
        remember_automatic_preference();
    }
    continue_anyway
}

/// Enforce discrete GPU policy or exit if user declines to continue
///
/// This function:
/// 1. Probes for discrete GPUs
/// 2. If found, logs the adapter the high-performance preference will pick
///    (the preference itself reaches the renderer through `WgpuOptions`)
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

    let preference = engine_state::settings::global_config()
        .get(
            engine_state::settings::NS_EDITOR,
            "renderer",
            "gpu_preference",
        )
        .ok()
        .and_then(|value| value.as_str().ok().map(str::to_owned))
        .unwrap_or_else(|| "high_performance".to_owned());
    if preference == "low_power" || preference == "auto" {
        tracing::info!(%preference, "GPU policy uses the configured non-discrete preference");
        return;
    }

    let probe = probe_gpu_policy();

    if probe.has_discrete_gpu {
        // The preference reaches the renderer through `WgpuOptions` (built
        // in `main` from the same setting). Nothing reads the
        // `WGPU_POWER_PREF`/`WGPU_ADAPTER_NAME` variables this used to set,
        // and setting them here raced the runtime's worker threads: `setenv`
        // against a `getenv` from the logging layer is a data race on Unix.
        tracing::info!(
            adapter = probe.selected_gpu_name.as_deref().unwrap_or("unknown"),
            "Discrete GPU detected; the high-performance GPU preference applies"
        );
        return;
    }

    tracing::warn!("No discrete GPU detected; prompting user for continue/exit decision");
    if !prompt_continue_without_discrete_gpu(&probe.adapters) {
        tracing::error!("Startup aborted by user because no discrete GPU is available");
        std::process::exit(1);
    }

    tracing::warn!("User chose to continue without a discrete GPU");
}

#[cfg(test)]
mod tests {
    use super::{counts_as_discrete, decide, prompt_request, CONTINUE, QUIT};
    use pulsar_dialog_protocol::{DialogResponse, Outcome};
    use wgpu::DeviceType;

    #[test]
    fn any_adapter_named_rtx_spark_counts_as_discrete_however_it_is_reported() {
        let name = "NVIDIA RTX Spark N1X (6144-core Blackwell RTX GPU)";
        assert!(counts_as_discrete(DeviceType::IntegratedGpu, name));
        assert!(counts_as_discrete(DeviceType::Other, &format!("{name}/PCIe")));
        assert!(counts_as_discrete(DeviceType::Other, "nvidia rtx spark"));
    }

    #[test]
    fn ordinary_discrete_cards_still_count_and_other_integrated_gpus_do_not() {
        assert!(counts_as_discrete(DeviceType::DiscreteGpu, "AMD Radeon RX"));
        assert!(!counts_as_discrete(DeviceType::IntegratedGpu, "AMD Radeon Graphics"));
        assert!(!counts_as_discrete(DeviceType::IntegratedGpu, "Qualcomm Adreno X1"));
        assert!(!counts_as_discrete(DeviceType::Cpu, "Microsoft Basic Render Driver"));
        // The name has to say RTX Spark: "Spark" alone is not enough.
        assert!(!counts_as_discrete(DeviceType::IntegratedGpu, "Spark Graphics"));
    }

    #[test]
    fn continue_carries_on_and_remembers_only_when_the_box_is_ticked() {
        assert_eq!(decide(&DialogResponse::answered(CONTINUE, Some(false))), (true, false));
        assert_eq!(decide(&DialogResponse::answered(CONTINUE, None)), (true, false));
        assert_eq!(decide(&DialogResponse::answered(CONTINUE, Some(true))), (true, true));
    }

    #[test]
    fn quit_and_closing_the_dialog_decline_and_never_remember() {
        assert_eq!(decide(&DialogResponse::answered(QUIT, Some(true))), (false, false));
        assert_eq!(decide(&DialogResponse::dismissed(Some(true))), (false, false));
    }

    #[test]
    fn no_dialog_at_all_carries_on_as_a_headless_machine_always_did() {
        let silence = DialogResponse::unavailable();
        assert_eq!(silence.outcome, Outcome::Unavailable);
        assert_eq!(decide(&silence), (true, false));
    }

    #[test]
    fn the_prompt_continues_on_enter_quits_on_escape_and_lists_the_adapters() {
        let request = prompt_request(&["Adreno X1 (integrated GPU)".to_owned()]);
        assert_eq!(request.default_button().unwrap().id, CONTINUE);
        assert_eq!(request.cancel_button().unwrap().id, QUIT);
        assert!(request.detail.as_deref().unwrap().contains("Adreno X1"));
        assert!(request.checkbox.as_ref().is_some_and(|c| !c.checked));
        assert!(prompt_request(&[]).detail.is_none());
    }
}
