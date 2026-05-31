//! Project file association management
//!
//! This module handles associating Pulsar project files (Pulsar.toml) with the
//! current engine executable. It provides platform-specific implementations for:
//! - Windows: Registry-based file associations
//! - macOS: UTI-based associations via duti
//! - Linux: Desktop entry and MIME type associations

use crate::consts;
use file_association_manager::{AssociationError, AssociationRequest, FileAssociationManager};

pub const PROJECT_ASSOC_EXTENSION: &str = "Pulsar.toml";
pub const PROJECT_ASSOC_MIME: &str = "application/x-pulsar-project";

#[cfg(target_os = "macos")]
const MACOS_BUNDLE_ID: &str = "dev.pulsar.engine";
#[cfg(target_os = "macos")]
const MACOS_ASSOC_QUERY_EXTENSION: &str = "toml";

/// Prompt the user to associate Pulsar project files with this engine build
///
/// This function:
/// 1. Checks if the file association manager is available
/// 2. Determines if the association is already set correctly
/// 3. Prompts the user to create the association if needed
/// 4. Applies the association using platform-specific methods
pub fn maybe_prompt_project_file_association() {
    let manager = match FileAssociationManager::system() {
        Ok(manager) => manager,
        Err(AssociationError::ToolMissing(tool)) => {
            tracing::warn!(
                "File association tooling is missing on this machine: {}",
                tool
            );

            let message = if cfg!(target_os = "macos") && tool == "duti" {
                "Pulsar could not check or set project file associations because 'duti' is not installed.\n\nInstall it with:\n  brew install duti\n\nThen relaunch Pulsar to enable one-click association for Pulsar.toml."
            } else {
                "Pulsar could not check or set project file associations because a required tool is missing.\n\nInstall the required association tool and relaunch Pulsar."
            };

            let _ = rfd::MessageDialog::new()
                .set_title("Pulsar Project Association")
                .set_description(message)
                .set_level(rfd::MessageLevel::Warning)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return;
        }
        Err(err) => {
            tracing::debug!("Skipping file association check: {}", err);
            return;
        }
    };

    let request = match build_project_association_request() {
        Some(req) => req,
        None => {
            tracing::debug!("No file association request is available for this platform");
            #[cfg(target_os = "macos")]
            {
                let _ = rfd::MessageDialog::new()
                    .set_title("Pulsar Project Association")
                    .set_description(
                        "Pulsar could not determine a valid TOML UTI on this macOS installation, so association was skipped.",
                    )
                    .set_level(rfd::MessageLevel::Warning)
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
            }
            return;
        }
    };

    let expected_handler = request.handler_id.trim().to_ascii_lowercase();
    let already_associated = manager
        .query(association_query_target())
        .ok()
        .flatten()
        .map(|record| record.handler_id.trim().to_ascii_lowercase() == expected_handler)
        .unwrap_or(false);

    if already_associated {
        tracing::debug!(
            "Project descriptor association already points to this engine handler ({})",
            request.handler_id
        );
        return;
    }

    let should_associate = rfd::MessageDialog::new()
        .set_title("Associate Pulsar Project Files")
        .set_description(format!(
            "Pulsar can associate project descriptor files ({}) with this running engine build (v{}).\n\nOn macOS this is applied via the TOML UTI mapping.\n\nAssociate now?",
            PROJECT_ASSOC_EXTENSION,
            consts::ENGINE_VERSION,
        ))
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();

    if !matches!(should_associate, rfd::MessageDialogResult::Yes) {
        tracing::debug!("User declined Pulsar project file association prompt");
        return;
    }

    match manager.set(request) {
        Ok(()) => {
            tracing::info!("Project descriptor file association updated successfully");
            let _ = rfd::MessageDialog::new()
                .set_title("Pulsar Project Association")
                .set_description("Pulsar project descriptor association was updated for this engine build.")
                .set_level(rfd::MessageLevel::Info)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
        Err(err) => {
            tracing::warn!("Failed to update project file association: {}", err);
            let _ = rfd::MessageDialog::new()
                .set_title("Pulsar Project Association")
                .set_description(format!(
                    "Pulsar could not update file associations automatically.\n\n{}",
                    err
                ))
                .set_level(rfd::MessageLevel::Warning)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
    }
}

/// Build platform-specific association request
fn build_project_association_request() -> Option<AssociationRequest> {
    let exe_path = std::env::current_exe().ok()?;

    #[cfg(target_os = "windows")]
    {
        let handler_id = format!(
            "dev.pulsar.engine.{}",
            consts::ENGINE_VERSION.replace('.', "_")
        );
        let command = format!("\"{}\" \"%1\"", exe_path.display());
        return Some(
            AssociationRequest::new(PROJECT_ASSOC_EXTENSION, handler_id)
                .with_mime_type(PROJECT_ASSOC_MIME)
                .with_command(command),
        );
    }

    #[cfg(target_os = "macos")]
    {
        let uti = detect_macos_toml_uti()?;
        return Some(
            AssociationRequest::new(uti, MACOS_BUNDLE_ID)
                .with_mime_type(PROJECT_ASSOC_MIME),
        );
    }

    #[cfg(target_os = "linux")]
    {
        let handler_id = ensure_linux_desktop_entry(&exe_path)?;
        return Some(
            AssociationRequest::new(PROJECT_ASSOC_EXTENSION, handler_id)
                .with_mime_type(PROJECT_ASSOC_MIME),
        );
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Get the platform-specific query target for checking existing associations
fn association_query_target() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        return MACOS_ASSOC_QUERY_EXTENSION;
    }

    #[cfg(not(target_os = "macos"))]
    {
        PROJECT_ASSOC_EXTENSION
    }
}

/// Detect the TOML UTI on macOS by creating a temporary .toml file and querying it
#[cfg(target_os = "macos")]
fn detect_macos_toml_uti() -> Option<String> {
    let probe_path = std::env::temp_dir().join(format!(
        "pulsar-assoc-probe-{}-{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos(),
        MACOS_ASSOC_QUERY_EXTENSION
    ));

    if std::fs::write(&probe_path, b"").is_err() {
        return None;
    }

    let output = std::process::Command::new("mdls")
        .args([
            "-raw",
            "-name",
            "kMDItemContentType",
            &probe_path.to_string_lossy(),
        ])
        .output()
        .ok()?;

    let _ = std::fs::remove_file(&probe_path);

    if !output.status.success() {
        return None;
    }

    let uti = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('"')
        .to_string();

    if uti.is_empty() || uti == "(null)" || uti.contains('/') || uti.ends_with(".app") {
        return None;
    }

    Some(uti)
}

/// Ensure a .desktop entry exists for Linux file associations
#[cfg(target_os = "linux")]
fn ensure_linux_desktop_entry(exe_path: &std::path::Path) -> Option<String> {
    let base_dirs = directories::BaseDirs::new()?;
    let desktop_dir = base_dirs.data_dir().join("applications");
    std::fs::create_dir_all(&desktop_dir).ok()?;

    let desktop_file_name = format!(
        "pulsar-engine-{}.desktop",
        consts::ENGINE_VERSION.replace('.', "-")
    );
    let desktop_path = desktop_dir.join(&desktop_file_name);

    let escaped_exe = exe_path.display().to_string().replace('"', "\\\"");
    let desktop_content = format!(
        "[Desktop Entry]\nType=Application\nName=Pulsar Engine\nExec=\"{}\" %f\nTerminal=false\nMimeType={};\nCategories=Development;IDE;\n",
        escaped_exe, PROJECT_ASSOC_MIME
    );

    let current = std::fs::read_to_string(&desktop_path).unwrap_or_default();
    if current != desktop_content {
        std::fs::write(&desktop_path, desktop_content).ok()?;
    }

    let _ = std::process::Command::new("update-desktop-database")
        .arg(&desktop_dir)
        .output();

    Some(desktop_file_name)
}
