//! System information gathering

use sysinfo::System;
use std::sync::Arc;
use parking_lot::RwLock;

/// Comprehensive system information
#[derive(Clone)]
pub struct SystemInfo {
    // OS Information
    pub os_name: String,
    pub os_version: String,
    pub kernel_version: String,
    pub host_name: String,

    // CPU Information
    pub cpu_brand: String,
    pub cpu_vendor: String,
    pub cpu_cores: usize,
    pub cpu_frequency: u64, // MHz

    // Memory Information
    pub total_memory: u64, // bytes
    pub total_swap: u64,   // bytes

    // GPU Information (from engine if available)
    pub gpu_name: String,
    pub gpu_driver_version: String,
    pub gpu_vendor: String,

    // Additional system info
    pub uptime: u64, // seconds
}

impl SystemInfo {
    pub fn gather() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();

        // OS Information
        let os_name = System::name().unwrap_or_else(|| "Unknown".to_string());
        let os_version = System::os_version().unwrap_or_else(|| "Unknown".to_string());
        let kernel_version = System::kernel_version().unwrap_or_else(|| "Unknown".to_string());
        let host_name = System::host_name().unwrap_or_else(|| "Unknown".to_string());

        // CPU Information
        let cpu_brand = sys.cpus().first()
            .map(|cpu| cpu.brand().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let cpu_vendor = sys.cpus().first()
            .map(|cpu| cpu.vendor_id().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let cpu_cores = sys.cpus().len();

        let cpu_frequency = sys.cpus().first()
            .map(|cpu| cpu.frequency())
            .unwrap_or(0);

        // Memory Information
        let total_memory = sys.total_memory();
        let total_swap = sys.total_swap();

        // GPU Information - will be populated from engine backend if available
        let gpu_name = "Detecting...".to_string();
        let gpu_driver_version = "N/A".to_string();
        let gpu_vendor = "N/A".to_string();

        // System uptime
        let uptime = System::uptime();

        Self {
            os_name,
            os_version,
            kernel_version,
            host_name,
            cpu_brand,
            cpu_vendor,
            cpu_cores,
            cpu_frequency,
            total_memory,
            total_swap,
            gpu_name,
            gpu_driver_version,
            gpu_vendor,
            uptime,
        }
    }

    pub fn total_memory_gb(&self) -> f64 {
        self.total_memory as f64 / 1024.0 / 1024.0 / 1024.0
    }

    pub fn total_swap_gb(&self) -> f64 {
        self.total_swap as f64 / 1024.0 / 1024.0 / 1024.0
    }

    pub fn uptime_formatted(&self) -> String {
        let days = self.uptime / 86400;
        let hours = (self.uptime % 86400) / 3600;
        let minutes = (self.uptime % 3600) / 60;

        if days > 0 {
            format!("{}d {}h {}m", days, hours, minutes)
        } else if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    }
}

/// Shared system info accessible across the application
pub type SharedSystemInfo = Arc<RwLock<SystemInfo>>;

/// Create a new shared system info instance
pub fn create_shared_info() -> SharedSystemInfo {
    Arc::new(RwLock::new(SystemInfo::gather()))
}
