//! Disk-backed thumbnail cache + shared background service for asset preview images.
//!
//! ## Architecture
//!
//! Each requested asset is represented as an editor task. Requests for the
//! same path are coalesced so callers share the same task and result.
//!
//! Consumers call [`service().request()`] which returns immediately.  When the
//! thumbnail is ready, the `on_done` callback receives the decoded
//! `Arc<image::RgbaImage>` (or `None` for unsupported types/failures).
//!
//! ## Layered cache
//!
//! 1. **Memory cache** — bounded LRU (up to [`MEM_CACHE_MAX`] entries).
//!    Entries expire after [`MEM_CACHE_TTL`].  A background eviction thread
//!    wakes every [`EVICTION_INTERVAL`] and prunes stale entries.
//!    Designed to hold 100 k+ *disk* entries while keeping RAM bounded.
//!
//! 2. **Disk cache** — `{cache_root}/.pulsar/thumbnails/{hash}.png`.
//!    Hash encodes path + mtime, so stale entries regenerate automatically.

use parking_lot::Mutex;
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Thumbnail output size in pixels (square).
const THUMB_PX: u32 = 128;
/// Bump when thumbnail output changes so stale cache entries regenerate.
const THUMBNAIL_RENDER_VERSION: u32 = 6;
/// Maximum number of decoded images held in the memory cache.
const MEM_CACHE_MAX: usize = 512;
/// How long an entry can go un-accessed before the eviction thread removes it.
const MEM_CACHE_TTL: Duration = Duration::from_secs(300); // 5 min
/// How often the background eviction thread wakes.
const EVICTION_INTERVAL: Duration = Duration::from_secs(60); // 1 min

// ─────────────────────────────────────────────────────────────────────────────
// In-memory LRU cache
// ─────────────────────────────────────────────────────────────────────────────

struct MemEntry {
    data: Arc<image::RgbaImage>,
    last_access: Instant,
}

/// Bounded, TTL-based in-memory cache keyed by the disk-cache hex key
/// (path + mtime hash).  Eviction is:
///   - LRU on insert when `len >= MEM_CACHE_MAX`
///   - TTL sweep by the background eviction thread
struct MemCache {
    entries: HashMap<String, MemEntry>,
}

impl MemCache {
    fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(MEM_CACHE_MAX),
        }
    }

    /// Fetch a decoded image, updating `last_access`.
    fn get(&mut self, key: &str) -> Option<Arc<image::RgbaImage>> {
        if let Some(e) = self.entries.get_mut(key) {
            e.last_access = Instant::now();
            Some(Arc::clone(&e.data))
        } else {
            None
        }
    }

    /// Insert, evicting the least-recently-used entry if at capacity.
    fn insert(&mut self, key: String, data: Arc<image::RgbaImage>) {
        if self.entries.len() >= MEM_CACHE_MAX && !self.entries.contains_key(&key) {
            // O(n) scan — n ≤ 512, negligible.
            if let Some(lru) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_access)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&lru);
            }
        }
        self.entries.insert(
            key,
            MemEntry {
                data,
                last_access: Instant::now(),
            },
        );
    }

    /// Remove all entries not accessed within `MEM_CACHE_TTL`.
    fn evict_expired(&mut self) {
        let cutoff = Instant::now()
            .checked_sub(MEM_CACHE_TTL)
            .unwrap_or(Instant::now());
        self.entries.retain(|_, e| e.last_access >= cutoff);
        // Shrink allocations once the cache has been heavily purged.
        if self.entries.capacity() > (self.entries.len() + 64).max(MEM_CACHE_MAX) {
            self.entries.shrink_to(MEM_CACHE_MAX);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public service API
// ─────────────────────────────────────────────────────────────────────────────

/// Global singleton worker.  Lazily started on first access.
static GLOBAL_SERVICE: OnceLock<ThumbnailService> = OnceLock::new();
pub type AssetThumbnailRenderer = fn(&Path) -> Option<image::RgbaImage>;
/// Backwards-compatible alias for existing mesh thumbnail providers.
pub type MeshThumbnailRenderer = AssetThumbnailRenderer;
static FORMAT_RENDERERS: OnceLock<Mutex<HashMap<String, AssetThumbnailRenderer>>> = OnceLock::new();

/// Register a renderer for a file extension (with or without a leading dot).
/// Plugin renderers run on the thumbnail worker thread and return a decoded
/// image; the shared service handles resizing and disk/memory caching.
pub fn register_thumbnail_renderer(extension: impl AsRef<str>, renderer: AssetThumbnailRenderer) {
    let extension = extension
        .as_ref()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if extension.is_empty() {
        return;
    }
    FORMAT_RENDERERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .insert(extension, renderer);
}

/// Register the editor's shared mesh renderer for common model formats.
/// Kept as a function pointer so this low-level filesystem crate does not
/// depend on the renderer crate.
pub fn register_mesh_thumbnail_renderer(renderer: MeshThumbnailRenderer) {
    for extension in [
        "fbx", "gltf", "glb", "obj", "usd", "usda", "usdc", "usdz", "uasset", "umap",
    ] {
        register_thumbnail_renderer(extension, renderer);
    }
}

/// Access the process-wide thumbnail service.
pub fn service() -> &'static ThumbnailService {
    GLOBAL_SERVICE.get_or_init(ThumbnailService::new)
}

/// Non-blocking thumbnail requests backed by the editor task queue and a
/// layered memory + disk cache.
pub struct ThumbnailService {
    /// Paths currently queued or being processed, with every caller waiting
    /// for the shared result.
    pending: Arc<Mutex<HashMap<PathBuf, Vec<ThumbnailCallback>>>>,
    /// Shared memory cache — written by the worker, read by `request()` on
    /// future calls once the asset is already cached.
    mem_cache: Arc<Mutex<MemCache>>,
}

type ThumbnailCallback = Box<dyn FnOnce(Option<Arc<image::RgbaImage>>) + Send + 'static>;

impl ThumbnailService {
    fn new() -> Self {
        let pending = Arc::new(Mutex::new(HashMap::<PathBuf, Vec<ThumbnailCallback>>::new()));
        let mem_cache = Arc::new(Mutex::new(MemCache::new()));
        // ── Background eviction thread ───────────────────────────────────────
        let evict_cache = Arc::clone(&mem_cache);
        std::thread::Builder::new()
            .name("thumbnail-evictor".into())
            .spawn(move || loop {
                std::thread::sleep(EVICTION_INTERVAL);
                let before = {
                    let mut c = evict_cache.lock();
                    let n = c.entries.len();
                    c.evict_expired();
                    n
                };
                let after = evict_cache.lock().entries.len();
                if before != after {
                    tracing::debug!(
                        "thumbnail mem-cache: evicted {} expired entries ({} remain)",
                        before - after,
                        after
                    );
                }
            })
            .expect("failed to spawn thumbnail-evictor thread");

        Self { pending, mem_cache }
    }

    /// Queue a thumbnail request.  Returns immediately (never blocks the caller).
    ///
    /// - Cached thumbnails are loaded without creating a task-queue entry.
    /// - Callers for the same uncached/in-flight path share one generation job
    ///   and each receive the resulting callback.
    /// - `on_done` receives the decoded `Arc<RgbaImage>`, or `None` if the type
    ///   is unsupported / generation failed.
    pub fn request(
        &self,
        abs_path: PathBuf,
        cache_root: PathBuf,
        on_done: impl FnOnce(Option<Arc<image::RgbaImage>>) + Send + 'static,
    ) {
        {
            let mut pending = self.pending.lock();
            if let Some(waiters) = pending.get_mut(&abs_path) {
                waiters.push(Box::new(on_done));
                return;
            }
            tracing::info!("thumbnail requested for {:?}", abs_path);
            pending.insert(abs_path.clone(), vec![Box::new(on_done)]);
        }

        let pending = Arc::clone(&self.pending);
        let mem_cache = Arc::clone(&self.mem_cache);
        let probe_mem_cache = Arc::clone(&mem_cache);
        let probe_path = abs_path.clone();
        let probe_root = cache_root.clone();
        smol::spawn(async move {
            let (cache_key, cached) = smol::unblock(move || {
                load_cached_thumbnail(&probe_path, &probe_root, &probe_mem_cache)
            })
            .await;
            if let Some(cached) = cached {
                complete_thumbnail(&pending, &abs_path, Some(cached));
                return;
            }

            let task_path = abs_path;
            let task_root = cache_root;
            let task_pending = Arc::clone(&pending);
            let task_mem_cache = Arc::clone(&mem_cache);
            let title = format!(
                "Generate thumbnail: {}",
                task_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("asset")
            );
            editor_task_queue::global().submit(
                editor_task_queue::TaskDescription::new(
                    title,
                    "Thumbnails",
                    editor_task_queue::TaskDuration::Long,
                ),
                move |task| {
                    let generated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if task.is_cancelled() {
                            return None;
                        }

                        tracing::info!("generating thumbnail for {:?}", task_path);
                        task.report_progress(0.1, "Rendering thumbnail");
                        let disk_path =
                            get_or_generate_thumbnail_sync(&task_path, &task_root, &cache_key);
                        let rgba = disk_path.and_then(|path| {
                            image::open(&path)
                                .map_err(|error| {
                                    tracing::debug!("thumbnail decode failed {:?}: {}", path, error)
                                })
                                .ok()
                                .map(|image| Arc::new(image.into_rgba8()))
                        });
                        if let Some(ref image) = rgba {
                            task_mem_cache.lock().insert(cache_key, Arc::clone(image));
                        }
                        rgba
                    }))
                    .unwrap_or_else(|_| {
                        tracing::error!("thumbnail task panicked for {:?}", task_path);
                        None
                    });

                    if generated.is_none() {
                        tracing::warn!("thumbnail generation failed for {:?}", task_path);
                    }
                    let succeeded = generated.is_some();
                    complete_thumbnail(&task_pending, &task_path, generated);
                    if task.is_cancelled() {
                        Err("Thumbnail generation cancelled".into())
                    } else if succeeded {
                        Ok(())
                    } else {
                        Err("Thumbnail generation failed".into())
                    }
                },
            );
        })
        .detach();
    }

    /// Returns the current number of entries in the memory cache.
    #[inline]
    pub fn mem_cache_len(&self) -> usize {
        self.mem_cache.lock().entries.len()
    }
}

fn complete_thumbnail(
    pending: &Mutex<HashMap<PathBuf, Vec<ThumbnailCallback>>>,
    path: &Path,
    image: Option<Arc<image::RgbaImage>>,
) {
    let waiters = pending.lock().remove(path).unwrap_or_default();
    for callback in waiters {
        callback(image.clone());
    }
}

fn load_cached_thumbnail(
    asset_path: &Path,
    cache_root: &Path,
    mem_cache: &Mutex<MemCache>,
) -> (String, Option<Arc<image::RgbaImage>>) {
    let cache_key = compute_cache_key(asset_path);
    if let Some(image) = mem_cache.lock().get(&cache_key) {
        return (cache_key, Some(image));
    }

    let cache_file = cache_root
        .join(".pulsar")
        .join("thumbnails")
        .join(format!("{cache_key}.png"));
    if !cache_file.exists() {
        return (cache_key, None);
    }

    match image::open(&cache_file) {
        Ok(image) => {
            let image = Arc::new(image.into_rgba8());
            mem_cache
                .lock()
                .insert(cache_key.clone(), Arc::clone(&image));
            (cache_key, Some(image))
        }
        Err(error) => {
            tracing::warn!(
                "thumbnail cache entry is corrupt; regenerating {:?}: {}",
                cache_file,
                error
            );
            let _ = std::fs::remove_file(cache_file);
            (cache_key, None)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sync cache logic (runs only on the worker thread)
// ─────────────────────────────────────────────────────────────────────────────

/// Return a path to a cached thumbnail PNG for `abs_asset_path`, generating it
/// if necessary.  This is the blocking implementation — call only from the
/// worker thread, never from the main / UI thread.
fn get_or_generate_thumbnail_sync(
    abs_asset_path: &Path,
    cache_root: &Path,
    cache_key: &str,
) -> Option<PathBuf> {
    let ext = abs_asset_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;

    if !is_supported_ext(&ext) {
        return None;
    }

    let cache_dir = cache_root.join(".pulsar").join("thumbnails");
    let cache_file = cache_dir.join(format!("{cache_key}.png"));

    // Fast path: already cached.
    if cache_file.exists() {
        if image::open(&cache_file).is_ok() {
            return Some(cache_file);
        }
        tracing::warn!(
            "thumbnail cache entry is corrupt; regenerating {:?}",
            cache_file
        );
        let _ = std::fs::remove_file(&cache_file);
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
        "fbx"
            | "gltf"
            | "glb"
            | "obj"
            | "usd"
            | "usda"
            | "usdc"
            | "usdz"
            | "uasset"
            | "umap"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "tga"
            | "bmp"
            | "gif"
            | "material"
    )
}

fn compute_cache_key(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();

    THUMBNAIL_RENDER_VERSION.hash(&mut hasher);
    path.hash(&mut hasher);

    // Modification time prevents stale previews when an asset is overwritten
    // with the same size and its changed bytes occur after the short fingerprint.
    if let Ok(meta) = std::fs::metadata(path) {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            modified.hash(&mut hasher);
        }
    }

    // Folder-backed formats (for example `.material`) give their registered
    // renderer the asset directory. Fingerprint its direct files so edits to
    // the marker/manifest invalidate the cached preview.
    if path.is_dir() {
        let mut entries = std::fs::read_dir(path)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            entry.file_name().hash(&mut hasher);
            if let Ok(metadata) = std::fs::metadata(&entry) {
                metadata.len().hash(&mut hasher);
                if let Ok(modified) = metadata.modified() {
                    modified.hash(&mut hasher);
                }
            }
            hash_file_prefix(&entry, &mut hasher);
        }
    } else {
        // Hash the first 8 KiB of content — fast fingerprint that is identical
        // for byte-for-byte duplicate files regardless of name or location.
        hash_file_prefix(path, &mut hasher);
    }

    // Model previews may use an adjacent BaseColor/Albedo/Diffuse map when an
    // importer omits the FBX texture connection. Include that dependency in the
    // key so replacing the image regenerates the thumbnail too.
    hash_base_color_sidecar(path, &mut hasher);

    format!("{:016x}", hasher.finish())
}

fn hash_file_prefix(path: &Path, hasher: &mut DefaultHasher) {
    use std::io::Read;

    if let Ok(mut file) = std::fs::File::open(path) {
        let mut buffer = [0u8; 8192];
        if let Ok(length) = file.read(&mut buffer) {
            buffer[..length].hash(hasher);
        }
    }
}

fn hash_base_color_sidecar(path: &Path, hasher: &mut DefaultHasher) {
    use std::io::Read;

    let (Some(parent), Some(stem)) = (
        path.parent(),
        path.file_stem().and_then(|stem| stem.to_str()),
    ) else {
        return;
    };
    let prefix = format!("{stem}_").to_lowercase();
    let mut candidates = std::fs::read_dir(parent)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let candidate = entry.path();
            let name = candidate.file_name()?.to_str()?.to_lowercase();
            let extension = candidate.extension()?.to_str()?.to_lowercase();
            if !name.starts_with(&prefix)
                || !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "tga")
            {
                return None;
            }
            let priority = if name.contains("basecolor") || name.contains("base_color") {
                0
            } else if name.contains("albedo") {
                1
            } else if name.contains("diffuse") {
                2
            } else {
                return None;
            };
            Some((priority, candidate))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    if let Some((_, sidecar)) = candidates.into_iter().next() {
        sidecar.hash(hasher);
        if let Ok(metadata) = std::fs::metadata(&sidecar) {
            metadata.len().hash(hasher);
            if let Ok(modified) = metadata.modified() {
                modified.hash(hasher);
            }
        }
        if let Ok(mut file) = std::fs::File::open(sidecar) {
            let mut buffer = [0u8; 8192];
            if let Ok(length) = file.read(&mut buffer) {
                buffer[..length].hash(hasher);
            }
        }
    }
}

fn generate_rgba(abs_path: &Path, ext: &str) -> Option<image::RgbaImage> {
    let mut rendered = if let Some(renderer) = FORMAT_RENDERERS
        .get()
        .and_then(|renderers| renderers.lock().get(ext).copied())
    {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| renderer(abs_path)))
            .unwrap_or_else(|_| {
                tracing::error!("thumbnail renderer panicked for {:?}", abs_path);
                None
            })
    } else {
        match ext {
            "png" | "jpg" | "jpeg" | "webp" | "tga" | "bmp" | "gif" => {
                let img = image::open(abs_path)
                    .map_err(|e| tracing::debug!("image load failed for {:?}: {}", abs_path, e))
                    .ok()?;
                Some(
                    img.resize(THUMB_PX, THUMB_PX, image::imageops::FilterType::Triangle)
                        .into_rgba8(),
                )
            }
            _ => {
                tracing::warn!("no thumbnail renderer registered for {:?}", abs_path);
                None
            }
        }
    }?;

    // Thumbnails are displayed over a uniform black canvas. Flatten alpha here
    // so transparent source images and renderer outputs use that same matte.
    for pixel in rendered.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        pixel[0] = ((u16::from(pixel[0]) * alpha + 127) / 255) as u8;
        pixel[1] = ((u16::from(pixel[1]) * alpha + 127) / 255) as u8;
        pixel[2] = ((u16::from(pixel[2]) * alpha + 127) / 255) as u8;
        pixel[3] = 255;
    }
    Some(rendered)
}

#[cfg(test)]
mod renderer_registration_tests {
    use super::{generate_rgba, register_thumbnail_renderer};
    use std::path::Path;

    fn test_renderer(_: &Path) -> Option<image::RgbaImage> {
        Some(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([12, 34, 56, 255]),
        ))
    }

    #[test]
    fn registered_extension_renderer_is_used_before_builtin_fallbacks() {
        register_thumbnail_renderer(".thumbnail-test", test_renderer);
        let image = generate_rgba(Path::new("asset.thumbnail-test"), "thumbnail-test")
            .expect("registered format renderer should produce an image");
        assert_eq!(image.get_pixel(0, 0).0, [12, 34, 56, 255]);
    }
}
