//! Dependency pre-flight checks.

use std::process::Command;

/// Returns `true` if a Rust toolchain is already reachable on PATH.
/// Everything else (build tools, SDKs, linkers) is handled by rustup itself.
pub fn check_rust() -> bool {
    Command::new("rustc").arg("--version").output().is_ok()
}
