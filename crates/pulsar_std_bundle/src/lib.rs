/// Pre-compiled `pulsar_std` native cdylib, embedded at build time.
///
/// The build script compiles `pulsar_std` as a `cdylib` for the host platform
/// and embeds the resulting bytes here. Every `#[blueprint]` function with a
/// numeric/bool signature exports a `__bp_dispatch_<name>` symbol that
/// `pulsar_bp_executor` resolves by name to build its dispatch table.
///
/// # Usage
///
/// ```rust,no_run
/// use pulsar_std_bundle::{PULSAR_STD_LIB_BYTES, extract_to_tempfile};
///
/// let lib = extract_to_tempfile().unwrap();
/// println!("lib path: {}", lib.path.display());
/// ```
pub const PULSAR_STD_LIB_BYTES: &[u8] = include_bytes!(env!("PULSAR_STD_LIB_PATH"));

/// Platform file extension of the embedded library (`"dylib"`, `"so"`, or `"dll"`).
pub const PULSAR_STD_LIB_EXT: &str = env!("PULSAR_STD_LIB_EXT");

/// Write the embedded library bytes to a temp file and return an RAII guard.
///
/// Keep the returned `TempLib` alive for the lifetime of any
/// `libloading::Library` loaded from it — dropping it deletes the temp file.
pub fn extract_to_tempfile() -> std::io::Result<TempLib> {
    use std::io::Write;
    let path = std::env::temp_dir()
        .join(format!("pulsar_std_bp.{}", PULSAR_STD_LIB_EXT));
    std::fs::File::create(&path)?.write_all(PULSAR_STD_LIB_BYTES)?;
    Ok(TempLib { path })
}

/// RAII guard that deletes the extracted temp library on drop.
pub struct TempLib {
    pub path: std::path::PathBuf,
}

impl Drop for TempLib {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.path); }
}
