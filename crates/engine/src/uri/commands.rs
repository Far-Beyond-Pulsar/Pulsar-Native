//! URI Command Definitions
//!
//! Defines the various commands that can be triggered via pulsar:// URIs

use std::path::PathBuf;

/// URI command enum for extensible URI handling
#[derive(Debug, Clone)]
pub enum UriCommand {
    /// Open a project directly
    /// Format: pulsar://open_project/url_encoded_path
    OpenProject { path: PathBuf },

    // Future commands can be added here:
    // /// Open a specific file within a project
    // /// Format: pulsar://open_file/project_path/file_path
    // OpenFile { project_path: PathBuf, file_path: PathBuf },
    //
    // /// Create a new project from a template
    // /// Format: pulsar://create_project/template_name/path
    // CreateProject { template: String, path: PathBuf },
}
