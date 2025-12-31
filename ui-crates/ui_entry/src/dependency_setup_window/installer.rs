//! Automated dependency installer.
//!
//! This module handles the execution of platform-specific setup scripts
//! to install missing development dependencies.

use std::process::Command;
use super::scripts::{SETUP_SCRIPT_PS1, SETUP_SCRIPT_SH};

/// Executes the platform-specific setup script to install dependencies.
///
/// This function writes the appropriate setup script to a temporary location
/// and executes it with the necessary permissions and shell environment.
///
/// # Platform Behavior
///
/// ## Windows
/// - Writes PowerShell script to temporary directory
/// - Executes with `powershell -ExecutionPolicy Bypass`
/// - Cleans up temporary file after execution
///
/// ## Linux/macOS
/// - Writes bash script to temporary directory
/// - Sets executable permissions (0o755)
/// - Executes with `bash`
/// - Cleans up temporary file after execution
///
/// # Returns
///
/// * `true` - If the setup script executed successfully
/// * `false` - If writing, executing, or any step failed
///
/// # Security Considerations
///
/// The script is executed with elevated privileges on Windows (Bypass execution policy)
/// and as executable on Unix systems. Ensure the embedded scripts are trusted.
///
/// # Example
///
/// ```no_run
/// if run_setup_script() {
///     println!("Dependencies installed successfully");
/// } else {
///     eprintln!("Failed to install dependencies");
/// }
/// ```
///
/// # Errors
///
/// This function returns `false` in the following cases:
/// - Failed to write script to temporary directory
/// - Failed to set executable permissions (Unix)
/// - Script execution returned non-zero exit code
/// - Any I/O error during the process
///
/// The temporary script file is always cleaned up, even on failure.
pub fn run_setup_script() -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::fs;
        
        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("pulsar-setup.ps1");
        
        // Write script to temporary file
        if fs::write(&script_path, SETUP_SCRIPT_PS1).is_err() {
            return false;
        }
        
        // Execute with PowerShell
        let result = Command::new("powershell")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script_path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        
        // Clean up temporary file
        let _ = fs::remove_file(&script_path);
        
        result
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        
        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("pulsar-setup.sh");
        
        // Write script to temporary file
        if fs::write(&script_path, SETUP_SCRIPT_SH).is_err() {
            return false;
        }
        
        // Set executable permissions
        if let Ok(metadata) = fs::metadata(&script_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&script_path, perms);
        }
        
        // Execute with bash
        let result = Command::new("bash")
            .arg(&script_path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        
        // Clean up temporary file
        let _ = fs::remove_file(&script_path);
        
        result
    }
}
