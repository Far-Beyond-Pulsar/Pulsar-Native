//! Disk-backed thumbnail cache + shared background service for asset preview images.
//!
//! ## Architecture
//!
//! A single OS thread (`thumbnail-worker`) drains a bounded job queue.  This
//! serialises GPU renders (helio-snapshot is wgpu-heavy) and ensures no two
//! callers accidentally kick off parallel renders for the same asset.
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
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Thumbnail output size in pixels (square).
const THUMB_PX: u32 = 128;
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

/// Access the process-wide thumbnail service.
pub fn service() -> &'static ThumbnailService {
    GLOBAL_SERVICE.get_or_init(ThumbnailService::new)
}

/// A non-blocking thumbnail request queue backed by a single worker thread
/// and a layered memory + disk cache.
pub struct ThumbnailService {
    sender: std::sync::mpsc::SyncSender<ThumbnailJob>,
    /// Paths currently queued or being processed — used for deduplication.
    pending: Arc<Mutex<HashSet<PathBuf>>>,
    /// Shared memory cache — written by the worker, read by `request()` on
    /// future calls once the asset is already cached.
    mem_cache: Arc<Mutex<MemCache>>,
}

struct ThumbnailJob {
    abs_path: PathBuf,
    cache_root: PathBuf,
    pending: Arc<Mutex<HashSet<PathBuf>>>,
    mem_cache: Arc<Mutex<MemCache>>,
    on_done: Box<dyn FnOnce(Option<Arc<image::RgbaImage>>) + Send + 'static>,
}

impl ThumbnailService {
    fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<ThumbnailJob>(128);
        let pending = Arc::new(Mutex::new(HashSet::<PathBuf>::new()));
        let mem_cache = Arc::new(Mutex::new(MemCache::new()));

        // ── Worker thread ────────────────────────────────────────────────────
        std::thread::Builder::new()
            .name("thumbnail-worker".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let cache_key = compute_cache_key(&job.abs_path);

                    // 1. Memory cache hit — no disk I/O needed.
                    let cached = job.mem_cache.lock().get(&cache_key);
                    if let Some(img) = cached {
                        job.pending.lock().remove(&job.abs_path);
                        (job.on_done)(Some(img));
                        continue;
                    }

                    // 2. Disk cache hit or generate.
                    let disk_path = get_or_generate_thumbnail_sync(&job.abs_path, &job.cache_root);

                    // 3. Decode once, cache in memory.
                    let rgba = disk_path.and_then(|p| {
                        image::open(&p)
                            .map_err(|e| tracing::debug!("thumbnail decode failed {:?}: {}", p, e))
                            .ok()
                            .map(|i| Arc::new(i.into_rgba8()))
                    });

                    if let Some(ref img) = rgba {
                        job.mem_cache.lock().insert(cache_key, Arc::clone(img));
                    }

                    job.pending.lock().remove(&job.abs_path);
                    (job.on_done)(rgba);
                }
            })
            .expect("failed to spawn thumbnail-worker thread");

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

        Self {
            sender: tx,
            pending,
            mem_cache,
        }
    }

    /// Queue a thumbnail request.  Returns immediately (never blocks the caller).
    ///
    /// - If the asset is already queued/in-flight the call is a no-op.
    /// - If the worker queue is full the pending flag is cleared so the caller
    ///   can retry on the next interaction.
    /// - `on_done` is invoked on the worker thread with the decoded
    ///   `Arc<RgbaImage>`, or `None` if the type is unsupported / generation
    ///   failed.
    pub fn request(
        &self,
        abs_path: PathBuf,
        cache_root: PathBuf,
        on_done: impl FnOnce(Option<Arc<image::RgbaImage>>) + Send + 'static,
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
            mem_cache: Arc::clone(&self.mem_cache),
            on_done: Box::new(on_done),
        };

        if self.sender.try_send(job).is_err() {
            pending_arc.lock().remove(&key);
        }
    }

    /// Returns the current number of entries in the memory cache.
    #[inline]
    pub fn mem_cache_len(&self) -> usize {
        self.mem_cache.lock().entries.len()
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
        "fbx"
            | "gltf"
            | "glb"
            | "obj"
            | "usd"
            | "usda"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "tga"
            | "bmp"
            | "gif"
    )
}

fn compute_cache_key(path: &Path) -> String {
    use std::io::Read;

    let mut hasher = DefaultHasher::new();

    // Hash file size so empty files don't collide with each other.
    if let Ok(meta) = std::fs::metadata(path) {
        meta.len().hash(&mut hasher);
    }

    // Hash the first 8 KiB of content — fast fingerprint that is identical for
    // byte-for-byte duplicate files regardless of their name or location.
    if let Ok(mut f) = std::fs::File::open(path) {
        let mut buf = [0u8; 8192];
        if let Ok(n) = f.read(&mut buf) {
            buf[..n].hash(&mut hasher);
        }
    }

    format!("{:016x}", hasher.finish())
}

fn generate_rgba(abs_path: &Path, ext: &str) -> Option<image::RgbaImage> {
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
        _ => None,
    }
}
