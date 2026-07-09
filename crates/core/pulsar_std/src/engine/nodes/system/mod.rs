//! # System Module
//!
//! System information and environment operations for the Pulsar visual programming system.
//!
//! This module provides system-level information including:
//! - Operating system details (OS, architecture, family)
//! - System identification (hostname, username)
//! - Directory paths (home, temp, executable path)
//! - Hardware information (CPU count, memory)
//! - Runtime information (uptime, command-line arguments)

use crate::blueprint;

// =============================================================================
// Operating System Information
// =============================================================================

/// Get the operating system name.
///
/// # Returns
/// The name of the operating system as a string (e.g., "windows", "macos", "linux")
///
/// # Example
/// On Windows, the output will be "windows".
/// On macOS, the output will be "macos".
/// On Linux, the output will be "linux".
///
/// # Notes
/// The output values are determined by Rust's `std::env::consts::OS`.
/// # System OS
/// Returns the operating system name (windows, macos, linux, etc.).
#[blueprint(type: NodeTypes::pure, category: "System", color: "#9B59B6")]
pub fn system_os() -> String {
    std::env::consts::OS.to_string()
}

/// Get the system architecture.
///
/// # Returns
/// The system architecture as a string (e.g., "x86_64", "arm", "aarch64")
///
/// # Example
/// On a typical 64-bit PC, the output will be "x86_64".
///
/// # Notes
/// Useful for diagnostics, logging, or platform-specific logic.
/// # System Arch
/// Returns the system architecture (x86_64, arm, etc.).
#[blueprint(type: NodeTypes::pure, category: "System", color: "#9B59B6")]
pub fn system_arch() -> String {
    std::env::consts::ARCH.to_string()
}

/// Get the operating system family.
///
/// # Returns
/// The operating system family as a string (e.g., "unix", "windows", "wasm")
///
/// # Example
/// On Windows, the output will be "windows".
/// On Linux or macOS, the output will be "unix".
///
/// # Notes
/// This is determined at compile time based on Rust's `std::env::consts::FAMILY`.
/// # System Family
/// Returns the operating system family (unix, windows, wasm).
#[blueprint(type: NodeTypes::pure, category: "System", color: "#9B59B6")]
pub fn system_family() -> String {
    std::env::consts::FAMILY.to_string()
}

// =============================================================================
// System Identification
// =============================================================================

/// Get the system hostname.
///
/// # Returns
/// The system hostname if successful, or an error message if retrieval fails
///
/// # Example
/// On most systems, the output will be the computer's network name (e.g., "DESKTOP-1234" or "my-macbook").
///
/// # Notes
/// This implementation uses the `hostname` command and works on both Windows and Unix-like systems.
/// # System Hostname
/// Returns the system hostname (network name of the machine).
#[blueprint(type: NodeTypes::fn_, category: "System", color: "#9B59B6")]
pub fn system_hostname() -> Result<String, String> {
    use std::process::Command;

    let output = if cfg!(target_os = "windows") {
        Command::new("hostname").output()
    } else {
        Command::new("hostname").output()
    };

    match output {
        Ok(output) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                Err("Failed to get hostname".to_string())
            }
        }
        Err(e) => Err(format!("Failed to execute hostname command: {}", e)),
    }
}

/// Get the current username.
///
/// # Returns
/// The username as a string, or None if it cannot be determined
///
/// # Example
/// On Windows, this will typically return the value of the `USERNAME` environment variable.
/// On Unix-like systems, it will return the value of the `USER` environment variable.
///
/// # Notes
/// If neither environment variable is set, the output will be None.
/// # System Username
/// Returns the current username from environment variables.
#[blueprint(type: NodeTypes::pure, category: "System", color: "#9B59B6")]
pub fn system_username() -> Option<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
}

// =============================================================================
// Directory Paths
// =============================================================================

/// Get the user's home directory.
///
/// # Returns
/// The path to the user's home directory, or None if not found
///
/// # Example
/// On Linux, the output might be Some("/home/username").
/// On Windows, the output might be Some("C:\\Users\\username").
///
/// # Notes
/// Checks the "HOME" environment variable (Unix-like systems) or "USERPROFILE" (Windows).
/// # System Home Dir
/// Returns the path to the user's home directory.
#[blueprint(type: NodeTypes::pure, category: "System", color: "#9B59B6")]
pub fn system_home_dir() -> Option<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
}

/// Get the system temporary directory.
///
/// # Returns
/// The path to the system temporary directory as a string
///
/// # Example
/// On Linux, the output might be "/tmp".
/// On Windows, it might be "C:\\Users\\<username>\\AppData\\Local\\Temp".
///
/// # Notes
/// The returned path is platform-dependent.
/// # System Temp Dir
/// Returns the path to the system temporary directory.
#[blueprint(type: NodeTypes::pure, category: "System", color: "#9B59B6")]
pub fn system_temp_dir() -> String {
    std::env::temp_dir().to_string_lossy().to_string()
}

/// Get the path to the current executable.
///
/// # Returns
/// The absolute path to the current executable, or an error message if retrieval fails
///
/// # Example
/// The output might be "C:\\Program Files\\MyApp\\myapp.exe" on Windows,
/// or "/usr/local/bin/myapp" on Unix systems.
///
/// # Notes
/// The result is platform-dependent and may vary based on how the application was launched.
/// # System Exe Path
/// Returns the path to the currently running executable.
#[blueprint(type: NodeTypes::fn_, category: "System", color: "#9B59B6")]
pub fn system_exe_path() -> Result<String, String> {
    match std::env::current_exe() {
        Ok(path) => Ok(path.to_string_lossy().to_string()),
        Err(e) => Err(format!("Failed to get executable path: {}", e)),
    }
}

// =============================================================================
// Hardware Information
// =============================================================================

/// Get the number of CPU cores.
///
/// # Returns
/// The number of logical CPU cores available
///
/// # Example
/// On a quad-core system with hyperthreading, the output might be 8.
///
/// # Notes
/// If the number of CPUs cannot be determined, this function returns 1 as a fallback.
/// # System CPU Count
/// Returns the number of logical CPU cores available.
#[blueprint(type: NodeTypes::pure, category: "System", color: "#9B59B6")]
pub fn system_cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1)
}

/// Get system memory information.
///
/// # Returns
/// A string describing the system's memory status, or an error message if retrieval fails
///
/// # Example
/// On Linux, the output may look like the result of the `free -h` command.
/// On Windows, it may include lines like "Total Physical Memory" and "Available Physical Memory".
///
/// # Notes
/// The format and detail of the output depend on the OS and available system commands.
/// # System Memory
/// Returns system memory information as a formatted string.
#[blueprint(type: NodeTypes::fn_, category: "System", color: "#9B59B6")]
pub fn system_memory() -> Result<String, String> {
    use std::process::Command;

    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "systeminfo | findstr \"Physical Memory\""])
            .output()
    } else if cfg!(target_os = "macos") {
        Command::new("vm_stat").output()
    } else {
        Command::new("free").args(["-h"]).output()
    };

    match output {
        Ok(output) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                Err("Failed to get memory info".to_string())
            }
        }
        Err(e) => Err(format!("Failed to execute memory command: {}", e)),
    }
}

// =============================================================================
// Runtime Information
// =============================================================================

/// Get the system uptime.
///
/// # Returns
/// The uptime or boot time as a string, or an error message if retrieval fails
///
/// # Example
/// On Unix: "up 1 day, 2:34"
/// On Windows: "System Boot Time: 5/1/2024, 8:00:00 AM"
///
/// # Notes
/// The output format may vary depending on the operating system and system locale.
/// # System Uptime
/// Returns the system uptime or boot time as a string.
#[blueprint(type: NodeTypes::fn_, category: "System", color: "#9B59B6")]
pub fn system_uptime() -> Result<String, String> {
    use std::process::Command;

    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "systeminfo | findstr \"Boot Time\""])
            .output()
    } else {
        Command::new("uptime").output()
    };

    match output {
        Ok(output) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                Err("Failed to get uptime".to_string())
            }
        }
        Err(e) => Err(format!("Failed to execute uptime command: {}", e)),
    }
}

/// Get command line arguments.
///
/// # Returns
/// A vector containing each command line argument as a string
///
/// # Example
/// If the program is run as `myapp.exe foo bar`, the output will be `["myapp.exe", "foo", "bar"]`.
///
/// # Notes
/// The first argument is usually the executable path.
/// # System Args
/// Returns command line arguments passed to the current process.
#[blueprint(type: NodeTypes::pure, category: "System", color: "#9B59B6")]
pub fn system_args() -> Vec<String> {
    std::env::args().collect()
}
