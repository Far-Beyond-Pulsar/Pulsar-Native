//! Provisions the `vg_stress` scan meshes (feature `vg-stress` only).

#[cfg(feature = "vg-stress")]
use std::{env, fs, path::Path};

fn main() {
    #[cfg(feature = "vg-stress")]
    provision_vg_stress(&env::var("CARGO_MANIFEST_DIR").unwrap());
}

// ---------------------------------------------------------------------------
// `vg_stress` scan provisioning (feature `vg-stress` only).
//
// A build script cannot see which binary is being built, so the example is
// gated behind a cargo feature: `cargo vg-stress` (alias) enables it, other
// examples and default builds never compile or run any of this.
// ---------------------------------------------------------------------------

#[cfg(feature = "vg-stress")]
#[path = "vg_stress_prep/mod.rs"]
mod vg_stress_prep;

#[cfg(feature = "vg-stress")]
fn provision_vg_stress(manifest_dir: &str) {
    println!("cargo:rerun-if-env-changed=HELIO_VG_STRESS_SKIP_PROVISION");
    println!("cargo:rerun-if-changed=vg_stress_prep");
    let assets = Path::new(manifest_dir).join("assets/vg_stress");
    // Watching the directory re-runs the script if a mesh is deleted.
    println!("cargo:rerun-if-changed={}", assets.join("meshes").display());
    if env::var_os("HELIO_VG_STRESS_SKIP_PROVISION").is_some() {
        return;
    }
    if vg_stress_prep::is_provisioned(&assets) {
        return;
    }
    if env::var("PROFILE").as_deref() != Ok("release") {
        println!(
            "cargo:warning=vg_stress scans are not prepared; skipping in a debug build \
             (use `cargo vg-stress`, or the demo prepares them when it starts)"
        );
        return;
    }
    let say = |m: &str| println!("cargo:warning=vg_stress: {m}");
    // A failed download must not break compilation: the demo retries at startup.
    if let Err(error) = vg_stress_prep::provision(&assets, &say) {
        println!("cargo:warning=vg_stress: provisioning failed ({error}); the demo will retry when it starts");
    }
}
