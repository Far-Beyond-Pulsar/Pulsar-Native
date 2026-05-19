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
}
