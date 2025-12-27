use std::process::Command;
use tracing;

fn main() {
    // Get the rustc version
    let output = Command::new("rustc")
        .arg("--version")
        .output()
        .expect("Failed to get rustc version");

    let version_str = String::from_utf8(output.stdout)
        .expect("rustc version output is not valid UTF-8");

    // Set as environment variable for compile-time access
    println!("cargo:rustc-env=RUSTC_VERSION={}", version_str.trim());

    // Rerun this build script if rustc version changes
    println!("cargo:rerun-if-changed=build.rs");
}
