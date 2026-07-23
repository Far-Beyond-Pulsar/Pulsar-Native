/// Build script for pulsar_std_bundle.
///
/// Compiles `pulsar_std` as a native cdylib and embeds the bytes.
/// Uses an isolated CARGO_TARGET_DIR so the subprocess never contends
/// the parent cargo's file lock.
#[path = "src/build_cache.rs"]
mod build_cache;

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../pulsar_std/src");
    println!("cargo:rerun-if-changed=../pulsar_std/Cargo.toml");
    println!("cargo:rerun-if-changed=../pulsar_macros/src");
    println!("cargo:rerun-if-changed=../pulsar_macros/Cargo.toml");
    println!("cargo:rerun-if-changed=../engine_fs/src");
    println!("cargo:rerun-if-changed=../engine_fs/Cargo.toml");
    println!("cargo:rerun-if-changed=../../../Cargo.lock");
    println!("cargo:rerun-if-env-changed=PULSAR_SKIP_NATIVE_BUILD");
    println!("cargo:rerun-if-env-changed=PULSAR_STD_NATIVE_TARGET_DIR");
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");
    println!("cargo:rerun-if-env-changed=RUSTFLAGS");
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");

    let ext = std::env::consts::DLL_EXTENSION;
    let prefix = if cfg!(target_os = "windows") {
        ""
    } else {
        "lib"
    };
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let dest = out_dir.join(format!("pulsar_std_native.{}", ext));

    println!("cargo:rustc-env=PULSAR_STD_LIB_EXT={}", ext);

    if std::env::var("PULSAR_SKIP_NATIVE_BUILD").is_ok() {
        std::fs::write(&dest, b"").unwrap();
        println!("cargo:rustc-env=PULSAR_STD_LIB_PATH={}", dest.display());
        println!("cargo:warning=Skipping native build (PULSAR_SKIP_NATIVE_BUILD set)");
        return;
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .ancestors()
        .nth(3)
        .expect("pulsar_std_bundle must live under crates/core")
        .to_path_buf();
    let pulsar_std_manifest = manifest_dir
        .parent()
        .unwrap()
        .join("pulsar_std")
        .join("Cargo.toml");

    // Use a stable target below the outer target root. It is isolated from the
    // parent Cargo lock, but unlike the previous OUT_DIR hash it survives build
    // script fingerprint changes and can reuse Cargo's dependency cache.
    let native_target = build_cache::native_target_dir(
        &workspace_root,
        std::env::var_os("PULSAR_STD_NATIVE_TARGET_DIR"),
        std::env::var_os("CARGO_TARGET_DIR"),
    );
    std::fs::create_dir_all(&native_target).expect("create native bundle target directory");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    let mut cmd = Command::new(&cargo);
    cmd.args([
        "build",
        "--manifest-path",
        pulsar_std_manifest.to_str().unwrap(),
        "--features",
        "native",
        "--no-default-features",
        "--locked",
    ])
    .env("CARGO_TARGET_DIR", &native_target)
    // Avoid inheriting Cargo's jobserver env from the parent build.
    // Child cargo gets its own scheduler and won't block waiting on parent tokens.
    .env_remove("CARGO_MAKEFLAGS")
    .env_remove("MAKEFLAGS")
    .env_remove("MFLAGS")
    .env_remove("NUM_JOBS");

    let built_profile = if profile == "release" {
        cmd.arg("--release");
        "release"
    } else if profile == "debug" {
        "debug"
    } else {
        cmd.args(["--profile", &profile]);
        &profile
    };

    let target = std::env::var("TARGET").expect("Cargo must set TARGET for build scripts");
    let host = std::env::var("HOST").expect("Cargo must set HOST for build scripts");
    let cross_target = (target != host).then_some(target.as_str());
    if let Some(target) = cross_target {
        cmd.args(["--target", target]);
    }

    let status = cmd.status().expect("failed to invoke cargo");

    if !status.success() {
        panic!("pulsar_std native build failed");
    }

    let built =
        build_cache::native_artifact_path(&native_target, cross_target, built_profile, prefix, ext);

    if !built.exists() {
        panic!("Expected dylib at {} — not found", built.display());
    }

    std::fs::copy(&built, &dest).expect("copy dylib");
    let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    println!("cargo:rustc-env=PULSAR_STD_LIB_PATH={}", dest.display());
    println!(
        "cargo:warning=pulsar_std native lib: {} bytes ({}) from {}",
        bytes,
        ext,
        display_path(&native_target)
    );
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
