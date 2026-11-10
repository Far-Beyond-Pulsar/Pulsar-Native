//! Shared error type and error-handling helpers for the Pulsar engine.
//!
//! # Error convention
//!
//! - **Library crates return typed errors** (`thiserror` enums such as
//!   [`EngineError`]), so callers can match on what went wrong.
//! - **`anyhow` is for binaries and UI glue only**, where an error is just
//!   reported to the user and never inspected.
//! - **Never panic on user input** (files, project data, plugin output,
//!   network). Return an error; reserve `unwrap`/`expect`/`panic!` for broken
//!   internal invariants, and say which invariant in the message.
//! - **Log with `tracing`** (`error!`, `warn!`, `info!`, ...), never
//!   `println!`/`eprintln!`.
//!
//! The macros in this crate ([`unwrap_or_warn!`], [`expect_or_log!`]) are
//! escape hatches that deliberately *swallow* an error after logging it. Read
//! their docs before using them.

use thiserror::Error;

#[doc(hidden)]
pub use tracing;

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Plugin load error: {0}")]
    PluginLoad(String),
    #[error("Runtime error: {0}")]
    Runtime(String),
    #[error("Unexpected error: {0}")]
    Unexpected(String),
}

/// Unwrap a `Result`, or log the error at `warn` level and evaluate to
/// `Default::default()` instead.
///
/// # Warning: this hides failures
///
/// The error is **not** propagated and the caller cannot tell that it
/// happened: on failure you silently get an empty/zero value
/// (`0`, `""`, `Vec::new()`, ...) that is indistinguishable from a real one.
/// Only use it where a default is genuinely a correct fallback and the failure
/// is not worth surfacing. Otherwise return the error with `?`.
///
/// The target type must implement [`Default`].
///
/// # Example
///
/// ```
/// use pulsar_error::unwrap_or_warn;
///
/// let ok: Result<u32, String> = Ok(7);
/// assert_eq!(unwrap_or_warn!(ok), 7);
///
/// // The error is logged and `0` is returned; the caller never sees the `Err`.
/// let failed: Result<u32, String> = Err("bad input".into());
/// assert_eq!(unwrap_or_warn!(failed), 0);
/// ```
#[macro_export]
macro_rules! unwrap_or_warn {
    ($expr:expr) => {{
        match $expr {
            Ok(v) => v,
            Err(e) => {
                $crate::tracing::warn!("unwrap_or_warn failed: {}", e);
                Default::default()
            }
        }
    }};
}

/// Unwrap a `Result`, or log `msg` plus the error at `error` level and
/// evaluate to `Default::default()` instead.
///
/// Like [`unwrap_or_warn!`] but with a caller-supplied message and a louder
/// log level, for failures that should be noticed in the logs.
///
/// # Warning: this hides failures
///
/// Despite the name, nothing is "expected" here: the error is **not**
/// propagated and execution continues with a default value
/// (`0`, `""`, `Vec::new()`, ...) that callers cannot tell apart from a real
/// one. Only use it where continuing with a default is genuinely correct;
/// otherwise return the error with `?`.
///
/// The target type must implement [`Default`].
///
/// # Example
///
/// ```
/// use pulsar_error::expect_or_log;
///
/// let failed: Result<Vec<u8>, String> = Err("disk unplugged".into());
/// // Logs "reading cache: disk unplugged" at error level, returns `vec![]`.
/// let bytes = expect_or_log!(failed, "reading cache");
/// assert!(bytes.is_empty());
/// ```
#[macro_export]
macro_rules! expect_or_log {
    ($expr:expr, $msg:expr) => {{
        match $expr {
            Ok(v) => v,
            Err(e) => {
                $crate::tracing::error!("{}: {}", $msg, e);
                Default::default()
            }
        }
    }};
}
