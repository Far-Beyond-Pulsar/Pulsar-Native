use std::path::Path;

fn emit_rerun_for_tree(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let child = entry.path();
            println!("cargo:rerun-if-changed={}", child.display());
            if child.is_dir() {
                emit_rerun_for_tree(&child);
            }
        }
    }
}

fn main() {
    // Windows: Embed application icon
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/images/logo_sqrkl.ico");

        if let Err(e) = res.compile() {
            eprintln!("Warning: Failed to embed icon: {}", e);
            // Don't fail the build if icon embedding fails
        }
    }

    eprintln!("cargo:rerun-if-changed=../../assets/images/logo_sqrkl_mac.ico");

    // Force a rebuild whenever the embedded default level changes so rust-embed
    // always bakes the latest version into the binary.
    println!("cargo:rerun-if-changed=../../assets/default.level");

    // Rebuild the engine when anything under assets/meshes changes so embedded
    // mesh assets stay up-to-date automatically.
    let meshes_root = Path::new("../../assets/meshes");
    if meshes_root.exists() {
        emit_rerun_for_tree(meshes_root);
    } else {
        println!("cargo:rerun-if-changed={}", meshes_root.display());
    }
}
