//! Cross-platform GPU detection.
//!
//! Uses `wgpu` adapter enumeration for name/vendor/device-type (works on all
//! platforms), then uses platform-specific APIs for driver version, total VRAM,
//! and live VRAM usage.

use wgpu::{Backends, Instance, InstanceDescriptor};

/// GPU information gathered at startup.
#[derive(Clone, Debug)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: String,
    pub driver_version: String,
    /// Total dedicated GPU memory in MiB.
    pub vram_total_mb: Option<u64>,
}

impl Default for GpuInfo {
    fn default() -> Self {
        Self {
            name: "Unknown".to_string(),
            vendor: "Unknown".to_string(),
            driver_version: "N/A".to_string(),
            vram_total_mb: None,
        }
    }
}

/// Detect the primary GPU. Prefers discrete over integrated GPUs.
pub fn detect_primary_gpu() -> GpuInfo {
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::all(),
        ..Default::default()
    });

    let adapters = instance.enumerate_adapters(Backends::all());

    // Prefer discrete GPU, then integrated, then anything else.
    let best = adapters.into_iter().max_by_key(|a| match a.get_info().device_type {
        wgpu::DeviceType::DiscreteGpu => 3,
        wgpu::DeviceType::IntegratedGpu => 2,
        wgpu::DeviceType::VirtualGpu => 1,
        _ => 0,
    });

    let Some(adapter) = best else {
        return GpuInfo::default();
    };

    let info = adapter.get_info();
    let vendor = pci_vendor_name(info.vendor as u32);
    let vram_total_mb = query_vram_total_mb();

    // wgpu populates driver/driver_info on Vulkan; on DX12 they are often empty.
    // Use platform-specific fallback when wgpu leaves them blank.
    let driver_version = {
        let wgpu_driver = if !info.driver.is_empty() && !info.driver_info.is_empty() {
            format!("{} ({})", info.driver, info.driver_info)
        } else if !info.driver.is_empty() {
            info.driver.clone()
        } else if !info.driver_info.is_empty() {
            info.driver_info.clone()
        } else {
            String::new()
        };

        if wgpu_driver.is_empty() {
            platform_driver_version(&info.name).unwrap_or_else(|| "N/A".to_string())
        } else {
            wgpu_driver
        }
    };

    GpuInfo {
        name: info.name,
        vendor,
        driver_version,
        vram_total_mb,
    }
}

/// Query current live VRAM usage in MiB. Called periodically by the metrics loop.
pub fn query_vram_used_mb() -> Option<u64> {
    platform_vram_used_mb()
}

fn pci_vendor_name(vendor_id: u32) -> String {
    match vendor_id as u32 {
        0x10DE => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        0x13B5 => "ARM",
        0x5143 => "Qualcomm",
        0x1010 => "ImgTec",
        _ => "Unknown",
    }
    .to_string()
}

// ═══════════════════════════════════════════════════════════════════
// Windows
// ═══════════════════════════════════════════════════════════════════

#[cfg(target_os = "windows")]
fn platform_driver_version(_gpu_name: &str) -> Option<String> {
    // Walk the Display class registry key and return the first DriverVersion found.
    use winreg::enums::*;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let class = hklm
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}")
        .ok()?;
    for i in 0u32..=16 {
        let name = format!("{:04}", i);
        if let Ok(sub) = class.open_subkey(&name) {
            if let Ok(ver) = sub.get_value::<String, _>("DriverVersion") {
                if !ver.is_empty() {
                    return Some(ver);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn query_vram_total_mb() -> Option<u64> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1};
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapters1(0).ok()?;
        let desc = adapter.GetDesc1().ok()?;
        let bytes = desc.DedicatedVideoMemory as u64;
        if bytes > 0 {
            Some(bytes / (1024 * 1024))
        } else {
            Some(desc.SharedSystemMemory as u64 / (1024 * 1024))
        }
    }
}

#[cfg(target_os = "windows")]
fn platform_vram_used_mb() -> Option<u64> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1,
        DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    };
    use windows::core::Interface;
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let adapter3: IDXGIAdapter3 = factory.EnumAdapters1(0).ok()?.cast().ok()?;
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        adapter3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).ok()?;
        Some(info.CurrentUsage / (1024 * 1024))
    }
}

// ═══════════════════════════════════════════════════════════════════
// Linux
// ═══════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
fn platform_driver_version(_gpu_name: &str) -> Option<String> {
    // NVIDIA: /proc/driver/nvidia/version
    if let Ok(content) = std::fs::read_to_string("/proc/driver/nvidia/version") {
        if let Some(line) = content.lines().next() {
            // "NVRM version: NVIDIA UNIX x86_64 Kernel Module  535.86.05  ..."
            if let Some(pos) = line.rfind("  ") {
                let ver = line[pos..].trim().split_whitespace().next().unwrap_or("").to_string();
                if !ver.is_empty() {
                    return Some(ver);
                }
            }
        }
    }
    // AMD/Mesa: read from sysfs driver module version
    if let Ok(content) = std::fs::read_to_string("/sys/module/amdgpu/version") {
        let ver = content.trim().to_string();
        if !ver.is_empty() {
            return Some(format!("amdgpu {}", ver));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn query_vram_total_mb() -> Option<u64> {
    let drm = std::path::Path::new("/sys/class/drm");
    for entry in std::fs::read_dir(drm).ok()?.flatten() {
        let path = entry.path().join("device/mem_info_vram_total");
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(bytes) = content.trim().parse::<u64>() {
                if bytes > 0 {
                    return Some(bytes / (1024 * 1024));
                }
            }
        }
    }
    nvidia_smi_query("memory.total")
}

#[cfg(target_os = "linux")]
fn platform_vram_used_mb() -> Option<u64> {
    // AMD: sysfs
    let drm = std::path::Path::new("/sys/class/drm");
    for entry in std::fs::read_dir(drm).ok()?.flatten() {
        let path = entry.path().join("device/mem_info_vram_used");
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(bytes) = content.trim().parse::<u64>() {
                if bytes > 0 {
                    return Some(bytes / (1024 * 1024));
                }
            }
        }
    }
    // NVIDIA
    nvidia_smi_query("memory.used")
}

#[cfg(target_os = "linux")]
fn nvidia_smi_query(field: &str) -> Option<u64> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu", field, "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().and_then(|l| l.trim().parse::<u64>().ok())
}

// ═══════════════════════════════════════════════════════════════════
// macOS
// ═══════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
fn platform_driver_version(_gpu_name: &str) -> Option<String> {
    // macOS doesn't expose a GPU driver version string easily.
    // Return the Metal GPU family from system_profiler as a proxy.
    let output = std::process::Command::new("system_profiler")
        .arg("SPDisplaysDataType")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Metal:") || t.starts_with("Metal Support:") {
            if let Some(val) = t.splitn(2, ':').nth(1) {
                return Some(val.trim().to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn query_vram_total_mb() -> Option<u64> {
    let output = std::process::Command::new("system_profiler")
        .arg("SPDisplaysDataType")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    parse_system_profiler_vram_field(&text, "vram")
}

#[cfg(target_os = "macos")]
fn platform_vram_used_mb() -> Option<u64> {
    // macOS doesn't expose live VRAM used via public APIs without IOKit.
    None
}

#[cfg(target_os = "macos")]
fn parse_system_profiler_vram_field(text: &str, field_prefix: &str) -> Option<u64> {
    for line in text.lines() {
        let trimmed = line.trim().to_lowercase();
        if trimmed.starts_with(field_prefix) && trimmed.contains(':') {
            let value = line.trim().splitn(2, ':').nth(1)?.trim();
            if let Some(pos) = value.to_uppercase().find("GB") {
                if let Ok(n) = value[..pos].trim().parse::<u64>() {
                    return Some(n * 1024);
                }
            } else if let Some(pos) = value.to_uppercase().find("MB") {
                if let Ok(n) = value[..pos].trim().parse::<u64>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════
// Fallback for unsupported platforms
// ═══════════════════════════════════════════════════════════════════

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn platform_driver_version(_gpu_name: &str) -> Option<String> { None }

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn query_vram_total_mb() -> Option<u64> { None }

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn platform_vram_used_mb() -> Option<u64> { None }

