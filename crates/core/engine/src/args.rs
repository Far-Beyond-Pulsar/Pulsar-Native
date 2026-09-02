//! Command-line argument and URI parsing for Pulsar Engine
//
// This module handles parsing of command-line arguments and URI launch commands.

use crate::uri;
use std::path::PathBuf;

const PROJECT_PATH_FLAG: &str = "--project-path";

/// Result of parsing command-line arguments.
#[derive(Clone)]
pub struct ParsedArgs {
    pub verbose: bool,
    pub project_path: Option<PathBuf>,
    pub uri_command: Option<uri::UriCommand>,
}

/// Parse command-line arguments and URI launch command.
pub fn parse_args() -> ParsedArgs {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    let project_path = parse_project_path(&args);
    let uri_command = uri::parse_launch_args().unwrap_or_default();
    ParsedArgs {
        verbose,
        project_path,
        uri_command,
    }
}

fn parse_project_path(args: &[String]) -> Option<PathBuf> {
    args.iter().enumerate().find_map(|(index, arg)| {
        if arg == PROJECT_PATH_FLAG {
            return args
                .get(index + 1)
                .filter(|value| !value.starts_with('-'))
                .map(PathBuf::from);
        }

        arg.strip_prefix("--project-path=").map(PathBuf::from)
    })
}

#[cfg(test)]
mod tests {
    use super::parse_project_path;
    use std::path::PathBuf;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parses_project_path_flag() {
        assert_eq!(
            parse_project_path(&args(&[
                "pulsar_engine",
                "--project-path",
                "C:/Projects/demo"
            ])),
            Some(PathBuf::from("C:/Projects/demo"))
        );
    }

    #[test]
    fn parses_project_path_equals_form() {
        assert_eq!(
            parse_project_path(&args(&["pulsar_engine", "--project-path=C:/Projects/demo"])),
            Some(PathBuf::from("C:/Projects/demo"))
        );
    }

    #[test]
    fn leaves_project_path_unset_without_flag() {
        assert_eq!(
            parse_project_path(&args(&["pulsar_engine", "--verbose"])),
            None
        );
    }

    #[test]
    fn leaves_project_path_unset_when_flag_has_no_value() {
        assert_eq!(
            parse_project_path(&args(&["pulsar_engine", "--project-path", "--verbose"])),
            None
        );
    }
}
