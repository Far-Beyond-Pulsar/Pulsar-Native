//! Disk-backed thumbnail cache + shared background service for asset preview images.
//!
//! ## Architecture
//!
//! A single OS thread (`thumbnail-worker`) drains a bounded job queue.  This
//! serialises GPU renders (helio-snapshot is wgpu-heavy) and ensures no two
//! callers accidentally kick off parallel renders for the same asset.
//!
//! Consumers — asset picker, content drawer, etc. — call [`service().request()`]
//! which returns immediately.  When the thumbnail is ready, the supplied
//! `on_done` callback is invoked on the worker thread with the path to the
//! cached PNG (or `None` for unsupported types).
//!
//! ## Disk cache
//!
//! Thumbnails are stored at `{cache_root}/.pulsar/thumbnails/{hash}.png`.
//! The hash encodes both the canonical asset path and its mtime, so stale
//! entries are automatically regenerated when a file changes on disk.

use parking_lot::Mutex;
use std::collections::{hash_map::DefaultHasher, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Thumbnail output size in pixels (square).
const THUMB_PX: u32 = 128;

// ─────────────────────────────────────────────────────────────────────────────
// Public service API
// ─────────────────────────────────────────────────────────────────────────────

/// Global singleton worker.  Lazily started on first access.
static GLOBAL_SERVICE: OnceLock<ThumbnailService> = OnceLock::new();

/// Access the process-wide thumbnail service.
pub fn service() -> &'static ThumbnailService {
    GLOBAL_SERVICE.get_or_init(ThumbnailService::new)
}

/// A non-blocking thumbnail request queue backed by a single worker thread.
pub struct ThumbnailService {
    sender: std::sync::mpsc::SyncSender<ThumbnailJob>,
    /// Paths currently queued or being processed — used for deduplication.
    pending: Arc<Mutex<HashSet<PathBuf>>>,
}

struct ThumbnailJob {
    abs_path: PathBuf,
    cache_root: PathBuf,
    pending: Arc<Mutex<HashSet<PathBuf>>>,
    on_done: Box<dyn FnOnce(Option<PathBuf>) + Send + 'static>,
}

impl ThumbnailService {
    fn new() -> Self {
        // Bounded to 128 slots; excess requests are silently dropped and can be
        // retried when the item scrolls back into view.
        let (tx, rx) = std::sync::mpsc::sync_channel::<ThumbnailJob>(128);
        let pending = Arc::new(Mutex::new(HashSet::new()));

        std::thread::Builder::new()
            .name("thumbnail-worker".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let result =
                        get_or_generate_thumbnail_sync(&job.abs_path, &job.cache_root);
                    // Release the in-flight lock before calling on_done so that
                    // a re-request from the callback doesn't deadlock.
                    job.pending.lock().remove(&job.abs_path);
                    (job.on_done)(result);
                }
            })
            .expect("failed to spawn thumbnail-worker thread");

        Self { sender: tx, pending }
    }

    /// Queue a thumbnail request.  Returns immediately (never blocks the caller).
    ///
    /// - If the asset is already queued/in-flight the call is a no-op.
    /// - If the worker queue is full the request is dropped; the pending flag is
    ///   cleared so the caller can retry on the next interaction.
    /// - `on_done` is invoked on the worker thread with the path to the cached
    ///   PNG, or `None` if the type is unsupported or generation failed.
    pub fn request(
        &self,
        abs_path: PathBuf,
        cache_root: PathBuf,
        on_done: impl FnOnce(Option<PathBuf>) + Send + 'static,
    ) {
        {
            let mut pending = self.pending.lock();
            if pending.contains(&abs_path) {
                return;
            }
            pending.insert(abs_path.clone());
        }

        let key = abs_path.clone();
        let pending_arc = Arc::clone(&self.pending);

        let job = ThumbnailJob {
            abs_path,
            cache_root,
            pending: Arc::clone(&self.pending),
            on_done: Box::new(on_done),
        };

        if self.sender.try_send(job).is_err() {
            // Queue full — clear the pending flag so the caller can retry.
            pending_arc.lock().remove(&key);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sync cache logic (runs only on the worker thread)
// ─────────────────────────────────────────────────────────────────────────────

/// Return a path to a cached thumbnail PNG for `abs_asset_path`, generating it
/// if necessary.  This is the blocking implementation — call only from the
/// worker thread, never from the main / UI thread.
fn get_or_generate_thumbnail_sync(abs_asset_path: &Path, cache_root: &Path) -> Option<PathBuf> {
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

    // Fast path: already cached.
    if cache_file.exists() {
        return Some(cache_file);
    }

    // Slow path: generate then persist.
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

fn compute_cache_key(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(mtime) = meta.modified() {
            mtime.hash(&mut hasher);
        }
    }
    format!("{:016x}", hasher.finish())
}

fn generate_rgba(abs_path: &Path, ext: &str) -> Option<image::RgbaImage> {
    match ext {
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
        "png" | "jpg" | "jpeg" | "webp" | "tga" | "bmp" | "gif" => {
            let img = image::open(abs_path)
                .map_err(|e| tracing::debug!("image load failed for {:?}: {}", abs_path, e))
                .ok()?;
            Some(
                img.resize(THUMB_PX, THUMB_PX, image::imageops::FilterType::Triangle)
                    .into_rgba8(),
            )
        }
        _ => None,
    }
}
