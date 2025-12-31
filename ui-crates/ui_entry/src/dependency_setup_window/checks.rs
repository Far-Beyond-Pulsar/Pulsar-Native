//! Platform-specific dependency checking functions.
//!
//! This module provides utilities to verify the presence of required
//! development tools and SDKs on different platforms.

use std::process::Command;

/// Checks if Rust toolchain is installed and accessible.
///
/// This verifies that `rustc` is available in the system PATH and can be executed.
///
/// # Returns
///
/// * `true` - If Rust is installed and `rustc --version` succeeds
/// * `false` - If Rust is not found or cannot be executed
///
/// # Example
///
/// ```no_run
/// if check_rust() {
///     println!("Rust is installed");
/// } else {
///     println!("Please install Rust from rustup.rs");
/// }
/// ```
pub fn check_rust() -> bool {
    Command::new("rustc")
        .arg("--version")
        .output()
        .is_ok()
}

/// Checks if platform-specific build tools are available.
///
/// This function verifies the presence of C/C++ compilers required for
/// building native dependencies:
///
/// - **Windows**: Checks for MSVC compiler (`cl.exe`)
/// - **Linux**: Checks for GCC compiler (`gcc`)
/// - **macOS**: Checks for Clang compiler (`clang`)
///
/// # Returns
///
/// * `true` - If the platform's C++ compiler is available
/// * `false` - If the compiler is not found
///
/// # Platform Notes
///
/// ## Windows
/// Requires Visual Studio Build Tools or full Visual Studio installation
/// with C++ development tools.
///
/// ## Linux
/// Requires `build-essential` package or equivalent for your distribution.
///
/// ## macOS
/// Requires Xcode Command Line Tools installed via `xcode-select --install`.
pub fn check_build_tools() -> bool {
    #[cfg(target_os = "windows")]
    {
        Command::new("cl")
            .arg("/?")
            .output()
            .is_ok()
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("gcc")
            .arg("--version")
            .output()
            .is_ok()
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("clang")
            .arg("--version")
            .output()
            .is_ok()
    }
}

/// Checks if platform-specific SDK and development tools are installed.
///
/// This function verifies the presence of platform SDKs and development
/// frameworks required for building native applications:
///
/// - **Windows**: Checks for Windows SDK in registry
/// - **Linux**: Checks for pkg-config utility
/// - **macOS**: Checks for Xcode Command Line Tools
///
/// # Returns
///
/// * `true` - If the platform SDK is available
/// * `false` - If the SDK is not found or inaccessible
///
/// # Platform Details
///
/// ## Windows
/// Queries the registry for Windows 10 SDK installation. The SDK is typically
/// installed with Visual Studio but can also be installed separately.
///
/// Registry key checked:
/// `HKLM\SOFTWARE\WOW6432Node\Microsoft\Microsoft SDKs\Windows\v10.0`
///
/// ## Linux
/// Verifies that `pkg-config` is installed, which is essential for finding
/// library compilation and linking flags.
///
/// ## macOS
/// Checks that Xcode Command Line Tools are installed by querying
/// `xcode-select -p` for the developer directory path.
pub fn check_platform_sdk() -> bool {
    #[cfg(target_os = "windows")]
    {
        Command::new("reg")
            .args(&["query", "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Microsoft SDKs\\Windows\\v10.0"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("xcode-select")
            .arg("-p")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("pkg-config")
            .arg("--version")
            .output()
            .is_ok()
    }
}
