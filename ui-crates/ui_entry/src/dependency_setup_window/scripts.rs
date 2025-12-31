//! Embedded setup scripts for dependency installation.
//!
//! This module contains platform-specific setup scripts that are embedded
//! directly into the binary at compile time. These scripts automate the
//! installation of missing dependencies.

/// PowerShell setup script for Windows platform.
///
/// This script is executed when dependencies are missing on Windows systems.
/// Currently a placeholder that directs users to run the manual setup script.
///
/// # Note
///
/// In production, this should be replaced with the actual content of
/// `script/setup-dev-environment.ps1` or implemented to dynamically
/// install dependencies via chocolatey or winget.
pub const SETUP_SCRIPT_PS1: &str = r#"
Write-Host "Pulsar Engine - Dependency Setup (PowerShell)"
Write-Host "This is a placeholder. Run script/setup-dev-environment.ps1 manually."
"#;

/// Bash setup script for Unix-like platforms (Linux/macOS).
///
/// This script is executed when dependencies are missing on Linux or macOS.
/// Currently a placeholder that directs users to run the manual setup script.
///
/// # Note
///
/// In production, this should be replaced with the actual content of
/// `script/setup-dev-environment.sh` or implemented to detect the package
/// manager (apt, yum, brew, etc.) and install dependencies accordingly.
pub const SETUP_SCRIPT_SH: &str = r#"#!/usr/bin/env bash
echo "Pulsar Engine - Dependency Setup (Bash)"
echo "This is a placeholder. Run script/setup-dev-environment.sh manually."
"#;
