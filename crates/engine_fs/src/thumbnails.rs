//! Disk-backed thumbnail cache for asset preview images.
//!
//! Thumbnails are stored at `{cache_root}/.pulsar/thumbnails/{hash}.png` where
//! the hash encodes the canonical asset path and its mtime so stale entries are
//! automatically replaced when a file changes on disk.
//!
//! Three asset categories are handled:
//! - **3D meshes** (fbx, gltf, glb, obj, usd, usda): rendered via `helio_snapshot`.
//! - **Image files** (png, jpg/jpeg, webp, tga, bmp, gif): copied/downsampled directly.
//! - **Everything else**: returns `None` — the UI shows a neutral placeholder.
//!
//! All functions are synchronous and intended to be called from a background thread.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Thumbnail output size in pixels (square).
const THUMB_PX: u32 = 128;

/// Return a path to a cached thumbnail PNG for `abs_asset_path`.
///
/// - `abs_asset_path`: absolute path to the source asset.
/// - `cache_root`: directory under which `.pulsar/thumbnails/` will be created.
///   Typically the open project root, or `std::env::current_dir()` for engine
///   built-in assets.
///
/// On first call the thumbnail is generated and saved to disk.  On subsequent
/// calls the cached file is returned immediately (unless the source has changed,
/// in which case the cache entry is regenerated).
///
/// Returns `None` if the asset type is not supported or generation fails.
pub fn get_or_generate_thumbnail(abs_asset_path: &Path, cache_root: &Path) -> Option<PathBuf> {
    let ext = abs_asset_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;

    if !is_supported_ext(&ext) {
        return None;
    }

    let cache_dir = cache_root.join(".pulsar").join("thumbnails");
    let cache_key = compute_cache_key(abs_asset_path);
    let cache_file = cache_dir.join(format!("{cache_key}.png"));

    // Fast path: cached file already exists and is up-to-date.
    if cache_file.exists() {
        return Some(cache_file);
    }

    // Slow path: generate thumbnail then write to disk.
    let rgba = generate_rgba(abs_asset_path, &ext)?;

    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        tracing::warn!("thumbnail cache: could not create {:?}: {}", cache_dir, e);
        return None;
    }

    if let Err(e) = rgba.save_with_format(&cache_file, image::ImageFormat::Png) {
        tracing::warn!("thumbnail cache: could not save {:?}: {}", cache_file, e);
        return None;
    }

    Some(cache_file)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

fn is_supported_ext(ext: &str) -> bool {
    matches!(
        ext,
        "fbx" | "gltf" | "glb" | "obj" | "usd" | "usda"
        | "png" | "jpg" | "jpeg" | "webp" | "tga" | "bmp" | "gif"
    )
}

/// Derive a stable cache key from the canonical path and the file's mtime.
/// Using `DefaultHasher` is fine here — collisions have negligible impact
/// (the worst case is a one-time thumbnail regeneration).
fn compute_cache_key(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    // Hash the canonical path string so cross-platform slashes don't matter.
    path.to_string_lossy().hash(&mut hasher);
    // Include mtime so stale cache entries are automatically replaced.
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(mtime) = meta.modified() {
            mtime.hash(&mut hasher);
        }
    }
    format!("{:016x}", hasher.finish())
}

/// Generate a 128×128 RGBA image for the asset, or return `None` on failure.
fn generate_rgba(abs_path: &Path, ext: &str) -> Option<image::RgbaImage> {
    match ext {
        // ── 3-D meshes: headless Helio render ─────────────────────────────
        "fbx" | "gltf" | "glb" | "obj" | "usd" | "usda" => {
            helio_snapshot::render_snapshot(
                abs_path,
                helio_snapshot::SnapshotConfig {
                    width: THUMB_PX,
                    height: THUMB_PX,
                    ..Default::default()
                },
            )
            .map_err(|e| tracing::debug!("snapshot failed for {:?}: {}", abs_path, e))
            .ok()
        }

        // ── Image files: decode and resize ────────────────────────────────
        "png" | "jpg" | "jpeg" | "webp" | "tga" | "bmp" | "gif" => {
            let img = image::open(abs_path)
                .map_err(|e| tracing::debug!("image load failed for {:?}: {}", abs_path, e))
                .ok()?;
            // Resize to thumbnail dimensions, preserving aspect ratio.
            Some(
                img.resize(THUMB_PX, THUMB_PX, image::imageops::FilterType::Triangle)
                    .into_rgba8(),
            )
        }

        _ => None,
    }
}
