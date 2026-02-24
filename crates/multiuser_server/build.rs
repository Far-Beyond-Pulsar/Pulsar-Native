use std::io::Result;

fn main() -> Result<()> {
    // Fix aws-lc-sys build on Windows by forcing C11 standard
    #[cfg(target_os = "windows")]
    {
        // Set multiple environment variables to force C11 compilation
        std::env::set_var("AWS_LC_SYS_C_STD", "c11");

        // Also set CFLAGS to directly pass /std:c11 to MSVC
        let current_cflags = std::env::var("CFLAGS").unwrap_or_default();
        let new_cflags = if current_cflags.is_empty() {
            "/std:c11".to_string()
        } else {
            format!("{} /std:c11", current_cflags)
        };
        std::env::set_var("CFLAGS", &new_cflags);
        std::env::set_var("CFLAGS_x86_64_pc_windows_msvc", &new_cflags);
        std::env::set_var("CFLAGS_x86_64-pc-windows-msvc", &new_cflags);

        // Also try setting it for aws-lc-sys specifically
        std::env::set_var("AWS_LC_SYS_CFLAGS", &new_cflags);

        // Set CMAKE flags since aws-lc-sys uses CMake
        std::env::set_var("CMAKE_C_STANDARD", "11");
        std::env::set_var("CMAKE_C_FLAGS", &new_cflags);

        // Force CMake to use the builder that respects these flags
        std::env::set_var("AWS_LC_SYS_CMAKE_BUILDER", "1");

        println!("cargo:warning=Setting CFLAGS for C11: {}", new_cflags);
        println!("cargo:warning=Setting CMAKE_C_STANDARD=11");
    }

    // TODO: Uncomment when proto files are created
    // prost_build::Config::new()
    //     .out_dir("src/proto")
    //     .compile_protos(&["proto/pulsar.proto"], &["proto/"])?;
    Ok(())
}
