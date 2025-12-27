//! URI Parsing
//!
//! Parses command-line arguments and pulsar:// URIs into structured commands

use std::env;
use std::path::PathBuf;
use anyhow::{Context, Result};
use urlencoding::decode;
use super::commands::UriCommand;

/// Parse command-line arguments for URI launch
///
/// Returns `Ok(Some(UriCommand))` if a valid URI is found,
/// `Ok(None)` if no URI is present,
/// `Err` if a URI is present but malformed
pub fn parse_launch_args() -> Result<Option<UriCommand>> {
    let args: Vec<String> = env::args().collect();

    // Check for URI in args (format: pulsar_engine.exe pulsar://...)
    for arg in args.iter().skip(1) {
        if arg.starts_with("pulsar://") {
            return parse_uri(arg).map(Some);
        }
    }

    Ok(None)
}

/// Parse a pulsar:// URI into a UriCommand
///
/// # Format
/// `pulsar://command/url_encoded_path`
///
/// # Example
/// `pulsar://open_project/C%3A%2FUsers%2Ftest%2Fproject`
///
/// # Errors
/// Returns error if:
/// - URI doesn't start with "pulsar://"
/// - URI format is invalid (missing command or path)
/// - Path cannot be decoded
/// - Path doesn't exist (for open_project)
/// - Path missing Pulsar.toml (for open_project)
/// - Command is unknown
pub fn parse_uri(uri: &str) -> Result<UriCommand> {
    if !uri.starts_with("pulsar://") {
        anyhow::bail!("Invalid URI scheme: expected 'pulsar://', got '{}'", uri);
    }

    // Extract command and path: pulsar://open_project/path
    let without_scheme = uri.strip_prefix("pulsar://")
        .context("Invalid URI format")?;

    let parts: Vec<&str> = without_scheme.splitn(2, '/').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid URI format: expected pulsar://command/path, got '{}'", uri);
    }

    let command = parts[0];
    let encoded_path = parts[1];

    // Decode URL-encoded path
    let decoded = decode(encoded_path)
        .context("Failed to decode URI path")?;
    let path = PathBuf::from(decoded.to_string());

    match command {
        "open_project" => {
            // Validate path exists
            if !path.exists() {
                anyhow::bail!("Project path does not exist: {:?}", path);
            }

            // Validate Pulsar.toml exists
            if !path.join("Pulsar.toml").exists() {
                anyhow::bail!("Not a valid Pulsar project (missing Pulsar.toml): {:?}", path);
            }

            Ok(UriCommand::OpenProject { path })
        }
        _ => anyhow::bail!("Unknown URI command: '{}'", command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_invalid_scheme() {
        let uri = "http://example.com";
        let result = parse_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid URI scheme"));
    }

    #[test]
    fn test_parse_malformed_uri() {
        let uri = "pulsar://invalid";
        let result = parse_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid URI format"));
    }

    #[test]
    fn test_parse_unknown_command() {
        let uri = "pulsar://unknown_command/path";
        let result = parse_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown URI command"));
    }

    #[test]
    fn test_parse_nonexistent_path() {
        let uri = "pulsar://open_project/nonexistent_path_12345";
        let result = parse_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_parse_valid_uri() {
        // Create a temporary project directory
        let temp_dir = TempDir::new().unwrap();
        let project_path = temp_dir.path();

        // Create Pulsar.toml
        fs::write(project_path.join("Pulsar.toml"), "").unwrap();

        // URL-encode the path
        let project_path_str = project_path.to_string_lossy().to_string();
        let encoded_path = urlencoding::encode(&project_path_str);
        let uri = format!("pulsar://open_project/{}", encoded_path);

        // Parse the URI
        let result = parse_uri(&uri);
        assert!(result.is_ok());

        match result.unwrap() {
            UriCommand::OpenProject { path } => {
                assert_eq!(path, project_path);
            }
        }
    }

    #[test]
    fn test_parse_uri_without_pulsar_toml() {
        // Create a temporary directory without Pulsar.toml
        let temp_dir = TempDir::new().unwrap();
        let project_path = temp_dir.path();

        // URL-encode the path
        let project_path_str = project_path.to_string_lossy();
        let encoded_path = urlencoding::encode(&project_path_str);
        let uri = format!("pulsar://open_project/{}", encoded_path);

        // Parse the URI - should fail
        let result = parse_uri(&uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing Pulsar.toml"));
    }
}
