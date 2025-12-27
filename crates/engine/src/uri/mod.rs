//! URI Scheme Handler
//!
//! Handles pulsar:// URI scheme registration and command parsing.
//! Supports extensible commands for launching projects and other operations.

pub mod parser;
pub mod commands;
pub mod registration;

pub use parser::parse_launch_args;
pub use commands::UriCommand;
pub use registration::ensure_uri_scheme_registered;
