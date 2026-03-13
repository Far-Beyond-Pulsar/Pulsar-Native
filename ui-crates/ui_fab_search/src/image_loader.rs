//! Background image downloader for Fab asset thumbnails and gallery images.
//!
//! Downloads image bytes via rquest + Chrome TLS impersonation and saves them
//! to a local disk cache under `{TEMP}/pulsar-fab-cache/`.  The cached path is
//! then passed directly to `gpui::img()` as a `PathBuf`, which GPUI loads with
//! a plain `fs::read` — no HTTP client registration required.

use std::path::PathBuf;

/// Return the per-user temporary cache directory for Fab images, creating it
/// on first call if needed.
fn cache_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("pulsar-fab-cache");
    println!("Using image cache directory: {}", dir.display());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Guess a file extension from the leading magic bytes.
fn ext_for_bytes(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, 0x50, 0x4e, 0x47, ..] => Some("png"),
        [0xff, 0xd8, ..] => Some("jpg"),
        [b'G', b'I', b'F', ..] => Some("gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("webp"),
        [b'B', b'M', ..] => Some("bmp"),
        _ => None,
    }
}

/// Derive a stable filename from the URL using a 64-bit FNV-like hash.
fn url_hash(url: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    h.finish()
}

/// Download `url`, save the bytes to the disk cache, and return the `PathBuf`.
///
/// If a file for this URL already exists on disk it is returned immediately
/// without re-downloading.  Always called from an OS thread.
pub fn download_to_cache(url: &str) -> Result<PathBuf, String> {
    use rquest::Impersonate;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let bytes = rt.block_on(async {
        let client = rquest::Client::builder()
            .impersonate(Impersonate::Chrome131)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;

        let response = client.get(url).send().await.map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| e.to_string())
    })?;

    let ext = ext_for_bytes(&bytes).unwrap_or("bin");
    let path = cache_dir()?.join(format!("{}.{}", url_hash(url), ext));

    // Skip writing if already on disk from a previous run.
    if !path.exists() {
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    }

    Ok(path)
}
