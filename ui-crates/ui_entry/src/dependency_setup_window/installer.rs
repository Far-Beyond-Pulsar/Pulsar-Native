//! Rust installer backed by the local rustup fork.
//!
//! Instead of writing placeholder shell scripts, this module delegates directly
//! to `rustup::installer::install_rust_blocking`, which runs the same platform-
//! native install flow as `rustup-init -y`. The call is made on a dedicated OS
//! thread so that the tokio runtime rustup spins up internally cannot conflict
//! with GPUI's executor.

/// Runs the rustup-based Rust installation unattended.
///
/// Spawns a dedicated thread (required because `install_rust_blocking`
/// constructs its own tokio multi-thread runtime). PATH modification is
/// enabled so `~/.cargo/bin` is wired up after install.
///
/// Returns `true` on success, `false` if installation failed for any reason.
pub fn run_setup_script() -> bool {
    let handle = std::thread::spawn(|| {
        rustup::installer::install_rust_blocking(
            /* no_prompt      */ true,
            /* no_modify_path */ false,
        )
        .is_ok()
    });

    handle.join().unwrap_or(false)
}
