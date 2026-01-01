//! URI Scheme Registration
//!
//! Registers the pulsar:// URI scheme with the operating system

use sysuri::{UriScheme, register, is_registered};
use std::env;
use anyhow::{Context, Result};

const SCHEME_NAME: &str = "pulsar";
const SCHEME_DESCRIPTION: &str = "Pulsar Engine Project Protocol";

/// Ensure the pulsar:// URI scheme is registered with the OS
///
/// Checks if the scheme is already registered before attempting registration
/// to avoid unnecessary system calls.
///
/// # Errors
/// Returns error if:
/// - Cannot get current executable path
/// - Registration fails
///
/// # Platform Support
/// - Windows: Registers in HKEY_CURRENT_USER (no admin required)
/// - macOS: Creates .app bundle in ~/Applications
/// - Linux: Creates .desktop file in ~/.local/share/applications/
pub fn ensure_uri_scheme_registered() -> Result<()> {
    tracing::debug!("Checking URI scheme registration...");

    // Check if already registered
    match is_registered(SCHEME_NAME) {
        Ok(true) => {
            tracing::debug!("✅ pulsar:// URI scheme already registered");
            return Ok(());
        }
        Ok(false) => {
            tracing::debug!("ℹ️  URI scheme not registered, registering now...");
        }
        Err(e) => {
            tracing::warn!("⚠️  Failed to check registration status: {}, attempting registration", e);
        }
    }

    // Get current executable path
    let exe_path = env::current_exe()
        .context("Failed to get executable path")?;

    tracing::debug!("Executable path: {:?}", exe_path);

    // Create URI scheme
    let scheme = UriScheme::new(
        SCHEME_NAME,
        SCHEME_DESCRIPTION,
        exe_path,
    );

    // Register with OS
    register(&scheme)
        .context("Failed to register URI scheme")?;

    tracing::debug!("✓ Successfully registered pulsar:// URI scheme");

    Ok(())
}
