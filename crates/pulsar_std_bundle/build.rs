/// Build script for pulsar_std_bundle.
///
/// Compiles `pulsar_std` as a native cdylib and embeds the bytes.
/// Uses an isolated CARGO_TARGET_DIR so the subprocess never contends
/// the parent cargo's file lock.
use std::path::PathBuf;
use std::process::Command;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

fn main() {
    println!("cargo:rerun-if-changed=../pulsar_std/src");
    println!("cargo:rerun-if-changed=../pulsar_std/Cargo.toml");
    println!("cargo:rerun-if-env-changed=PULSAR_SKIP_NATIVE_BUILD");

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

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let pulsar_std_manifest = PathBuf::from(&manifest_dir)
        .parent()
        .unwrap()
        .join("pulsar_std")
        .join("Cargo.toml");

    // Build into an isolated target dir outside the workspace target tree.
    // This avoids file-lock contention with the parent cargo invocation.
    let mut hasher = DefaultHasher::new();
    out_dir.hash(&mut hasher);
    let isolated_target =
        std::env::temp_dir().join(format!("pulsar_std_cdylib_target_{}", hasher.finish()));
    std::fs::create_dir_all(&isolated_target).ok();

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    let mut cmd = Command::new(&cargo);
    cmd.args([
        "build",
        "--manifest-path",
        pulsar_std_manifest.to_str().unwrap(),
        "--features",
        "native",
        "--no-default-features",
    ])
    .env("CARGO_TARGET_DIR", &isolated_target)
    // Avoid inheriting Cargo's jobserver env from the parent build.
    // Child cargo gets its own scheduler and won't block waiting on parent tokens.
    .env_remove("CARGO_MAKEFLAGS")
    .env_remove("MAKEFLAGS")
    .env_remove("MFLAGS")
    .env_remove("NUM_JOBS");

    let built_profile_dir = if profile == "release" {
        cmd.arg("--release");
        "release".to_string()
    } else if profile == "debug" {
        "debug".to_string()
    } else {
        cmd.args(["--profile", &profile]);
        profile.clone()
    };

    let status = cmd.status().expect("failed to invoke cargo");

    if !status.success() {
        panic!("pulsar_std native build failed");
    }

    let built = isolated_target
        .join(&built_profile_dir)
        .join(format!("{}pulsar_std.{}", prefix, ext));

    if !built.exists() {
        panic!("Expected dylib at {} — not found", built.display());
    }

    std::fs::copy(&built, &dest).expect("copy dylib");
    let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    println!("cargo:rustc-env=PULSAR_STD_LIB_PATH={}", dest.display());
    println!(
        "cargo:warning=pulsar_std native lib: {} bytes ({})",
        bytes, ext
    );
}
