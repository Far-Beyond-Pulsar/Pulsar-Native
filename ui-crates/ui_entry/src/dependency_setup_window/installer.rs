//! Rust installer backed by the local rustup fork.
//!
//! This module delegates to `rustup::installer::install_rust_blocking`, which
//! runs the same platform-native install flow as `rustup-init -y`.
//!
//! Threading note: this function is intentionally synchronous and blocking.
//! Callers must run it on a background thread — in GPUI that means wrapping
//! the call in `cx.background_executor().spawn(async { run_setup_script() })`.
//! Do NOT call it directly inside a `cx.spawn` closure.

/// Installs Rust via the rustup library unattended.
///
/// Blocks the calling thread until installation completes or fails.
/// PATH modification is enabled so `~/.cargo/bin` is wired up.
///
/// Returns `true` on success.
pub fn run_setup_script() -> bool {
    rustup::installer::install_rust_blocking(
        /* no_prompt      */ true,
        /* no_modify_path */ false,
    )
    .is_ok()
}
