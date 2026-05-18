/// Build script for pulsar_wasm_bundle.
///
/// Compiles `pulsar_std` for `wasm32-unknown-unknown --features wasm` using cargo,
/// then writes the resulting `.wasm` bytes to `$OUT_DIR/pulsar_std.wasm` so that
/// `src/lib.rs` can embed them with `include_bytes!`.
///
/// To skip the WASM build (e.g. in CI without wasm32 toolchain), set the env var:
///   PULSAR_SKIP_WASM_BUILD=1
///
/// The build will emit:
///   cargo:rustc-env=PULSAR_STD_WASM_PATH=<path>
/// so callers can also use `env!("PULSAR_STD_WASM_PATH")` at compile time.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Re-run if pulsar_std changes
    println!("cargo:rerun-if-changed=../pulsar_std/src");
    println!("cargo:rerun-if-changed=../pulsar_std/Cargo.toml");
    println!("cargo:rerun-if-env-changed=PULSAR_SKIP_WASM_BUILD");

    if std::env::var("PULSAR_SKIP_WASM_BUILD").is_ok() {
        write_stub_wasm();
        return;
    }

    // Ensure wasm32-unknown-unknown toolchain is available
    let rustup_check = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();

    let has_wasm32 = match rustup_check {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains("wasm32-unknown-unknown"),
        Err(_) => false,
    };

    if !has_wasm32 {
        eprintln!(
            "cargo:warning=wasm32-unknown-unknown target not installed. \
             Run `rustup target add wasm32-unknown-unknown` to enable WASM builds. \
             Using stub WASM module."
        );
        write_stub_wasm();
        return;
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let pulsar_std_manifest = PathBuf::from(&manifest_dir)
        .parent()
        .unwrap()
        .join("pulsar_std")
        .join("Cargo.toml");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let wasm_path = out_dir.join("pulsar_std.wasm");

    // Build pulsar_std as a cdylib for wasm32
    let status = Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            pulsar_std_manifest.to_str().unwrap(),
            "--target",
            "wasm32-unknown-unknown",
            "--features",
            "wasm",
            "--release",
            // Disable default features so `native` feature (rlua etc.) is excluded
            "--no-default-features",
        ])
        .status()
        .expect("failed to invoke cargo for wasm32 build");

    if !status.success() {
        panic!("pulsar_std wasm32 build failed — see cargo output above");
    }

    // Locate the output .wasm file.
    // cargo places it under the workspace target directory, not OUT_DIR.
    let workspace_root = PathBuf::from(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let built_wasm = workspace_root
        .join("target")
        .join("wasm32-unknown-unknown")
        .join("release")
        .join("pulsar_std.wasm");

    if !built_wasm.exists() {
        panic!(
            "Expected compiled WASM at {} but it was not found",
            built_wasm.display()
        );
    }

    // Copy to OUT_DIR where include_bytes! can reach it
    std::fs::copy(&built_wasm, &wasm_path).expect("failed to copy wasm artifact");

    println!("cargo:rustc-env=PULSAR_STD_WASM_PATH={}", wasm_path.display());
    println!(
        "cargo:warning=pulsar_std.wasm built ({} bytes)",
        std::fs::metadata(&wasm_path).map(|m| m.len()).unwrap_or(0)
    );
}

/// Write a minimal valid WASM module as a stub when the real build is skipped.
/// This keeps the crate compilable without the wasm32 toolchain installed.
fn write_stub_wasm() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let wasm_path = out_dir.join("pulsar_std.wasm");

    // Minimal wasm binary: magic + version + empty module
    let stub: &[u8] = &[
        0x00, 0x61, 0x73, 0x6d, // \0asm
        0x01, 0x00, 0x00, 0x00, // version 1
    ];
    std::fs::write(&wasm_path, stub).expect("failed to write stub wasm");
    println!("cargo:rustc-env=PULSAR_STD_WASM_PATH={}", wasm_path.display());
    println!("cargo:warning=Using stub (empty) pulsar_std.wasm — set up wasm32 toolchain for real builds");
}
