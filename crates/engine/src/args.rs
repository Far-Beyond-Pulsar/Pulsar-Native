//! Command-line argument and URI parsing for Pulsar Engine
//
// This module handles parsing of command-line arguments and URI launch commands.

use crate::uri;

/// Result of parsing command-line arguments.
#[derive(Clone)]
pub struct ParsedArgs {
    pub verbose: bool,
    pub uri_command: Option<uri::UriCommand>,
}

/// Parse command-line arguments and URI launch command.
pub fn parse_args() -> ParsedArgs {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    let uri_command = match uri::parse_launch_args() {
        Ok(cmd) => cmd,
        Err(_) => None,
    };
    ParsedArgs { verbose, uri_command }
}
