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
/// use pulsar_std_bundle::{PULSAR_STD_LIB_BYTES, extract_to_tempfile, expected_sha256};
///
/// let lib = extract_to_tempfile().unwrap();
/// let executor = pulsar_bp_executor::BpExecutor::load(&lib.path, Some(expected_sha256()));
/// ```
pub const PULSAR_STD_LIB_BYTES: &[u8] = include_bytes!(env!("PULSAR_STD_LIB_PATH"));

#[cfg(test)]
mod build_cache;

/// Return the expected SHA-256 digest of the embedded `pulsar_std` library.
///
/// This is computed lazily from the embeded bytes and cached for the
/// process lifetime. Pass the result to [`BpExecutor::load`] to guard
/// against TOCTOU replacement of the extracted temp file.
pub fn expected_sha256() -> &'static [u8; 32] {
    use once_cell::sync::OnceCell;
    use sha2::{Digest, Sha256};

    static HASH: OnceCell<[u8; 32]> = OnceCell::new();
    HASH.get_or_init(|| Sha256::digest(PULSAR_STD_LIB_BYTES).into())
}

/// Platform file extension of the embedded library (`"dylib"`, `"so"`, or `"dll"`).
pub const PULSAR_STD_LIB_EXT: &str = env!("PULSAR_STD_LIB_EXT");

static TEMP_LIB_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write the embedded library bytes to a temp file and return an RAII guard.
///
/// Keep the returned `TempLib` alive for the lifetime of any
/// `libloading::Library` loaded from it — dropping it deletes the temp file.
pub fn extract_to_tempfile() -> std::io::Result<TempLib> {
    use std::io::Write;
    use std::sync::atomic::Ordering;

    let mut path = std::env::temp_dir();
    let pid = std::process::id();

    let file = loop {
        let suffix = TEMP_LIB_COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!(
            "pulsar_std_bp_{pid}_{suffix}.{}",
            PULSAR_STD_LIB_EXT
        ));

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => break file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                path.pop();
            }
            Err(err) => return Err(err),
        }
    };

    let mut file = file;
    file.write_all(PULSAR_STD_LIB_BYTES)?;
    file.flush()?;
    Ok(TempLib { path })
}

/// RAII guard that deletes the extracted temp library on drop.
pub struct TempLib {
    pub path: std::path::PathBuf,
}

impl Drop for TempLib {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
