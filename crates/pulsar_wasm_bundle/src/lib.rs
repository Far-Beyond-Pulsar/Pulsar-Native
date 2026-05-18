/// Pre-compiled `pulsar_std` native dynamic library, embedded at build time.
///
/// The build script compiles `pulsar_std` as a `cdylib` for the host platform
/// and embeds the resulting bytes here. Every `#[blueprint]` function with a
/// numeric/bool signature exports a `__bp_dispatch_<name>` symbol that the
/// `pulsar_bp_executor` resolves by name to build its zero-enum dispatch table.
///
/// # Usage
///
/// ```rust,no_run
/// use pulsar_wasm_bundle::{PULSAR_STD_LIB_BYTES, PULSAR_STD_LIB_EXT, extract_to_tempfile};
///
/// // Write the embedded bytes to a temp file and get a path libloading can open:
/// let path = extract_to_tempfile().unwrap();
/// println!("lib path: {}", path.display());
/// ```
pub const PULSAR_STD_LIB_BYTES: &[u8] = include_bytes!(env!("PULSAR_STD_LIB_PATH"));

/// Platform file extension of the embedded library (`"dylib"`, `"so"`, or `"dll"`).
pub const PULSAR_STD_LIB_EXT: &str = env!("PULSAR_STD_LIB_EXT");

/// Write the embedded library bytes to a temporary file and return its path.
///
/// The caller is responsible for keeping the returned `TempLib` alive for as
/// long as any `libloading::Library` loaded from it is in use — dropping it
/// deletes the temp file.
pub fn extract_to_tempfile() -> std::io::Result<TempLib> {
    use std::io::Write;

    let dir = std::env::temp_dir();
    let name = format!("pulsar_std_bp.{}", PULSAR_STD_LIB_EXT);
    let path = dir.join(&name);
    let mut f = std::fs::File::create(&path)?;
    f.write_all(PULSAR_STD_LIB_BYTES)?;
    f.flush()?;
    Ok(TempLib { path })
}

/// RAII guard that deletes the extracted temp library when dropped.
pub struct TempLib {
    pub path: std::path::PathBuf,
}

impl Drop for TempLib {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
