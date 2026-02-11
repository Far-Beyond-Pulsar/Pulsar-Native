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

    println!("cargo:rerun-if-changed=../../assets/images/logo_sqrkl.ico");
}
