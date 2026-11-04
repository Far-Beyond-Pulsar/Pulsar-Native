//! Cook scheduling for the VG museum: memory-bounded concurrency, atomic cache
//! writes, progress/ETA formatting, and the small classic simplifier used for
//! the temporary alpha-masked foliage parts.
//!
//! Nothing here touches the GPU or the window: the same code runs inside the
//! loader thread and in the headless `--prepare` mode.

use helio_mesh_api::{MeshUpload, PackedVertex};
use helio_vg_build::{BuildOptions, MeshInput};
use helio_vg_stream::{content_words, VgeoBytes, VgeoSource};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    time::Duration,
};

// ── memory model ─────────────────────────────────────────────────────────────

/// Peak bytes per source triangle while a glTF/GLB is imported (decoded
/// accessors, converted vertices, indices, importer scratch). Conservative.
pub const IMPORT_BYTES_PER_TRI: u64 = 96;
/// Peak bytes per triangle of the part being cooked (position/attribute
/// copies, adjacency, cluster DAG scratch). Conservative.
pub const COOK_BYTES_PER_TRI: u64 = 200;
/// A group with at least this many source triangles is a "heavy" cook: only
/// one heavy cook runs at a time, whatever the memory budget says.
pub const HEAVY_TRIANGLES: u64 = 10_000_000;

const GIB: u64 = 1 << 30;

/// Estimated peak memory of one job: the whole file stays imported while the
/// largest of its parts cooks.
pub fn job_peak_bytes(file_triangles: u64, largest_part_triangles: u64) -> u64 {
    file_triangles * IMPORT_BYTES_PER_TRI + largest_part_triangles * COOK_BYTES_PER_TRI
}

pub fn is_heavy(file_triangles: u64) -> bool {
    file_triangles >= HEAVY_TRIANGLES
}

/// The cook memory budget in bytes. `env` is `VG_STRESS_COOK_MEM_GB` (an
/// explicit override, only floored at 0.25 GB); otherwise 60% of the memory
/// that is free right now, clamped to 2..24 GiB (8 GiB if unknown).
pub fn budget_bytes(env: Option<&str>, available: Option<u64>) -> u64 {
    if let Some(gb) = env
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
    {
        return ((gb * GIB as f64) as u64).max(GIB / 4);
    }
    match available {
        Some(free) => ((free as f64 * 0.6) as u64).clamp(2 * GIB, 24 * GIB),
        None => 8 * GIB,
    }
}

/// Physical memory that is free right now, where the platform tells us.
pub fn available_memory_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct MemoryStatusEx {
            length: u32,
            load: u32,
            total_phys: u64,
            avail_phys: u64,
            total_page: u64,
            avail_page: u64,
            total_virtual: u64,
            avail_virtual: u64,
            avail_extended: u64,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        }
        let mut status = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page: 0,
            avail_page: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended: 0,
        };
        // SAFETY: `status` is a correctly sized, initialised MEMORYSTATUSEX.
        return (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.avail_phys);
    }
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        return Some(kb * 1024);
    }
    #[allow(unreachable_code)]
    None
}

/// Cook threads: leave two cores to the render and loader threads.
pub fn cook_threads(env: Option<&str>, cpus: usize) -> usize {
    env.and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| cpus.saturating_sub(2).clamp(2, 8))
}

/// How many jobs may be in flight at all (the gate decides which may start).
pub fn job_workers(cpus: usize) -> usize {
    (cpus / 4).clamp(1, 3)
}

// ── the gate ─────────────────────────────────────────────────────────────────

#[derive(Default)]
struct GateState {
    used: u64,
    running: usize,
    heavy_running: bool,
}

/// Admits jobs so that the sum of their estimated peaks stays within the
/// budget and at most one heavy job runs. A job larger than the whole budget
/// is admitted when it can run alone, so nothing starves.
pub struct CookGate {
    state: Mutex<GateState>,
    changed: Condvar,
    budget: u64,
}

pub struct Permit<'a> {
    gate: &'a CookGate,
    bytes: u64,
    heavy: bool,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut s = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        s.used -= self.bytes;
        s.running -= 1;
        if self.heavy {
            s.heavy_running = false;
        }
        drop(s);
        self.gate.changed.notify_all();
    }
}

impl CookGate {
    pub fn new(budget: u64) -> Self {
        Self {
            state: Mutex::new(GateState::default()),
            changed: Condvar::new(),
            budget,
        }
    }

    pub fn budget(&self) -> u64 {
        self.budget
    }

    fn admit(&self, s: &mut GateState, bytes: u64, heavy: bool) -> Option<u64> {
        let charged = bytes.min(self.budget);
        let fits = s.running == 0 || s.used + charged <= self.budget;
        (fits && !(heavy && s.heavy_running)).then(|| {
            s.used += charged;
            s.running += 1;
            s.heavy_running |= heavy;
            charged
        })
    }

    pub fn try_acquire(&self, bytes: u64, heavy: bool) -> Option<Permit<'_>> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.admit(&mut s, bytes, heavy).map(|charged| Permit {
            gate: self,
            bytes: charged,
            heavy,
        })
    }

    pub fn acquire(&self, bytes: u64, heavy: bool) -> Permit<'_> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(charged) = self.admit(&mut s, bytes, heavy) {
                return Permit {
                    gate: self,
                    bytes: charged,
                    heavy,
                };
            }
            s = self.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn in_use(&self) -> (u64, usize) {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (s.used, s.running)
    }
}

/// A unit of cook work as the scheduler sees it.
#[derive(Clone, Debug)]
pub struct JobCost {
    pub peak_bytes: u64,
    pub heavy: bool,
}

/// Run `jobs` (smallest peak first, so the cheap assets show up first) on
/// `workers` threads through `gate`. `heartbeat` runs on the calling thread
/// about four times a second until everything has finished.
pub fn run_jobs<J: Send>(
    mut jobs: Vec<(JobCost, J)>,
    gate: &CookGate,
    workers: usize,
    run: &(dyn Fn(J) + Sync),
    heartbeat: &mut dyn FnMut(),
) {
    jobs.sort_by_key(|(cost, _)| cost.peak_bytes);
    let total = jobs.len();
    if total == 0 {
        return;
    }
    let queue = Mutex::new(jobs.into_iter().collect::<VecDeque<_>>());
    let finished = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for n in 0..workers.max(1).min(total) {
            let (queue, finished) = (&queue, &finished);
            let _ = std::thread::Builder::new()
                .name(format!("vg-cook-{n}"))
                .spawn_scoped(scope, move || {
                    loop {
                        let next = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
                        let Some((cost, job)) = next else { break };
                        let _permit = gate.acquire(cost.peak_bytes, cost.heavy);
                        // A panicking cook must not wedge the others or the count.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job)));
                        finished.fetch_add(1, Ordering::Relaxed);
                    }
                });
        }
        while finished.load(Ordering::Relaxed) < total {
            std::thread::sleep(Duration::from_millis(250));
            heartbeat();
        }
    });
}

// ── progress ─────────────────────────────────────────────────────────────────

pub fn format_duration(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, s % 3600 / 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Remaining seconds from the throughput so far; `None` until something has
/// finished (a half-cooked asset says nothing reliable about the rest).
pub fn eta_seconds(done_triangles: u64, total_triangles: u64, elapsed: f64) -> Option<f64> {
    (done_triangles > 0 && elapsed > 0.0).then(|| {
        total_triangles.saturating_sub(done_triangles) as f64 * elapsed / done_triangles as f64
    })
}

pub fn millions(t: u64) -> String {
    format!("{:.2}M", t as f64 / 1e6)
}

// ── files ────────────────────────────────────────────────────────────────────

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Write `bytes` to `path` so that a reader (or a killed run) only ever sees
/// the old file or the complete new one: unique temp file, flushed to disk,
/// then renamed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = temp_path(path);
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}-{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Remove temp files a killed run left behind (older than ten minutes, so a
/// second running instance is not disturbed).
pub fn clean_stale_temp_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "tmp")
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(600))
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn vgeo_path(cache: &Path, content: u128) -> PathBuf {
    cache.join(format!("{content:032x}.vgeo"))
}

/// `--force-recook` or `VG_STRESS_FORCE_RECOOK=1`: ignore every valid `.vgeo` cache
/// and cook each asset again. Applies once per asset per run (an asset cooked in
/// this run is read back normally), so it also works with `--prepare`.
pub fn force_recook() -> bool {
    static FORCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FORCE.get_or_init(|| {
        let on = std::env::args().skip(1).any(|a| a == "--force-recook")
            || std::env::var("VG_STRESS_FORCE_RECOOK")
                .is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false"));
        if on {
            println!("[Museum cook] --force-recook: every .vgeo cache is ignored and cooked again");
        }
        on
    })
}

/// Assets already cooked again in this run under [`force_recook`].
fn recooked() -> &'static Mutex<std::collections::HashSet<u128>> {
    static SET: std::sync::OnceLock<Mutex<std::collections::HashSet<u128>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(Default::default)
}

/// A cache file is valid when it parses, belongs to `content` and was made by
/// the current format version and cook revision (both live in its header).
/// Under [`force_recook`] it is not valid until this run has cooked it again.
pub fn read_vgeo(cache: &Path, content: u128) -> Option<Arc<VgeoBytes>> {
    read_vgeo_with(cache, content, force_recook())
}

fn read_vgeo_with(cache: &Path, content: u128, force: bool) -> Option<Arc<VgeoBytes>> {
    if force && !recooked().lock().unwrap().contains(&content) {
        return None;
    }
    // File-backed: the header is read now, pages as the streamer asks for them.
    // (Reading the whole file held ~4 GB of RAM for this scene.)
    let source = VgeoBytes::open(&vgeo_path(cache, content)).ok()?;
    (source.header().content_id == content_words(content)
        && helio_vg_build::is_current_cook(&source.header()))
    .then(|| Arc::new(source))
}

// ── cooking ──────────────────────────────────────────────────────────────────

pub fn stable_content_id(vertices: &[PackedVertex], indices: &[u32]) -> u128 {
    let mut low = 0xcbf29ce484222325u64;
    let mut high = 0x84222325cbf29ce4u64;
    let bytes = bytemuck::cast_slice::<PackedVertex, u8>(vertices);
    for byte in bytes
        .iter()
        .copied()
        .chain(bytemuck::cast_slice::<u32, u8>(indices).iter().copied())
    {
        low = (low ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        high = (high ^ u64::from(byte).rotate_left(1)).wrapping_mul(0x9e3779b185ebca87);
    }
    (u128::from(high) << 64) | u128::from(low)
}

pub fn pack_vgeo_attributes(vertices: &[PackedVertex]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vertices.len() * helio_vg_format::ATTR_STRIDE as usize);
    for vertex in vertices {
        let tangent = (vertex.tangent & 0x00ff_ffff)
            | ((if vertex.bitangent_sign < 0.0 {
                0x81u32
            } else {
                0x7fu32
            }) << 24);
        bytes.extend_from_slice(&vertex.normal.to_le_bytes());
        bytes.extend_from_slice(&tangent.to_le_bytes());
        for uv in [vertex.tex_coords0, vertex.tex_coords1] {
            bytes.extend_from_slice(&half::f16::from_f32(uv[0]).to_bits().to_le_bytes());
            bytes.extend_from_slice(&half::f16::from_f32(uv[1]).to_bits().to_le_bytes());
        }
    }
    bytes
}

/// The valid cache file for `content`, or a fresh cook written atomically.
/// Returns whether it was a cache hit.
pub fn load_or_build_vgeo_cache(
    content_id: u128,
    vertices: &[PackedVertex],
    indices: &[u32],
    cache: &Path,
) -> Result<(Arc<VgeoBytes>, bool), String> {
    if let Some(vgeo) = read_vgeo(cache, content_id) {
        return Ok((vgeo, true));
    }
    let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
    let attributes = pack_vgeo_attributes(vertices);
    let mesh = MeshInput {
        positions: &positions,
        attributes: &attributes,
        attribute_stride: helio_vg_format::ATTR_STRIDE as usize,
        indices: &indices[..indices.len() / 3 * 3],
        tri_material: &[],
    };
    let bytes = helio_vg_build::build_vgeo(
        &mesh,
        &BuildOptions {
            content_id: content_words(content_id),
            resident_levels: 2,
        },
    )
    .map_err(|error| error.to_string())?;
    drop((positions, attributes));
    write_atomic(&vgeo_path(cache, content_id), &bytes).map_err(|error| error.to_string())?;
    recooked().lock().unwrap().insert(content_id);
    drop(bytes);
    VgeoBytes::open(&vgeo_path(cache, content_id))
        .map(|vgeo| (Arc::new(vgeo), false))
        .map_err(|error| error.to_string())
}

/// A one-triangle, zero-area mesh. VG parts own no classic geometry; the
/// scene still needs a mesh row for the part, because the interned vertex
/// range of that row is the identity virtual geometry keys its assets by.
/// `ordinal` keeps every placeholder's bytes distinct.
pub fn placeholder_mesh(ordinal: u32) -> MeshUpload {
    let mut vertex = PackedVertex::default();
    vertex.tex_coords0 = [ordinal as f32, 0.0];
    MeshUpload {
        vertices: vec![vertex; 3],
        indices: vec![0, 1, 2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    #[test]
    fn budget_prefers_env_then_free_memory() {
        assert_eq!(budget_bytes(Some("6"), Some(100 * GIB)), 6 * GIB);
        assert_eq!(budget_bytes(Some("0.01"), None), GIB / 4);
        // Junk falls through to the automatic budget.
        assert_eq!(budget_bytes(Some("lots"), Some(10 * GIB)), 6 * GIB);
        assert_eq!(budget_bytes(None, Some(1 * GIB)), 2 * GIB);
        assert_eq!(budget_bytes(None, Some(200 * GIB)), 24 * GIB);
        assert_eq!(budget_bytes(None, None), 8 * GIB);
    }

    #[test]
    fn estimates_scale_with_triangles() {
        // Pine: a 17.2M triangle file, 7M largest variant.
        let pine = job_peak_bytes(17_182_252, 6_951_524);
        assert!(pine > 2 * GIB && pine < 6 * GIB, "{pine}");
        assert!(is_heavy(17_182_252) && is_heavy(10_000_000) && !is_heavy(7_000_000));
        assert!(job_peak_bytes(1_000_000, 1_000_000) < job_peak_bytes(2_000_000, 2_000_000));
    }

    #[test]
    fn gate_bounds_concurrent_memory() {
        let gate = CookGate::new(1000 * MIB);
        let a = gate.try_acquire(600 * MIB, false).expect("first fits");
        assert!(
            gate.try_acquire(600 * MIB, false).is_none(),
            "second would exceed the budget"
        );
        let b = gate.try_acquire(400 * MIB, false).expect("exactly fits");
        assert_eq!(gate.in_use(), (1000 * MIB, 2));
        drop(a);
        assert!(gate.try_acquire(700 * MIB, false).is_none());
        drop(b);
        assert_eq!(gate.in_use(), (0, 0));
    }

    #[test]
    fn oversized_job_runs_alone_and_blocks_others() {
        let gate = CookGate::new(1000 * MIB);
        let small = gate.try_acquire(10 * MIB, false).unwrap();
        assert!(
            gate.try_acquire(5000 * MIB, false).is_none(),
            "must wait for an empty gate"
        );
        drop(small);
        let big = gate
            .try_acquire(5000 * MIB, false)
            .expect("alone, charged at most the budget");
        assert_eq!(gate.in_use().0, 1000 * MIB);
        assert!(
            gate.try_acquire(1, false).is_none(),
            "nothing runs beside an over-budget job"
        );
        drop(big);
    }

    #[test]
    fn only_one_heavy_cook_at_a_time() {
        let gate = CookGate::new(100 * GIB);
        let heavy = gate.try_acquire(GIB, true).unwrap();
        assert!(
            gate.try_acquire(GIB, true).is_none(),
            "second 10M+ cook must wait despite free memory"
        );
        let light = gate
            .try_acquire(GIB, false)
            .expect("light jobs are not blocked");
        drop(heavy);
        let heavy2 = gate.try_acquire(GIB, true).expect("heavy slot freed");
        drop((light, heavy2));
    }

    #[test]
    fn scheduler_runs_smallest_first_within_budget() {
        use std::sync::atomic::AtomicU64;
        let gate = CookGate::new(100 * MIB);
        let order = Mutex::new(Vec::new());
        let peak = AtomicU64::new(0);
        let now = AtomicU64::new(0);
        let jobs: Vec<(JobCost, u64)> = [60u64, 10, 30, 20, 50, 40]
            .iter()
            .map(|&mb| {
                (
                    JobCost {
                        peak_bytes: mb * MIB,
                        heavy: false,
                    },
                    mb,
                )
            })
            .collect();
        run_jobs(
            jobs,
            &gate,
            3,
            &|mb| {
                let used = now.fetch_add(mb, Ordering::SeqCst) + mb;
                peak.fetch_max(used, Ordering::SeqCst);
                order.lock().unwrap().push(mb);
                std::thread::sleep(Duration::from_millis(20));
                now.fetch_sub(mb, Ordering::SeqCst);
            },
            &mut || {},
        );
        assert!(
            peak.load(Ordering::SeqCst) <= 100,
            "peak {} MiB",
            peak.load(Ordering::SeqCst)
        );
        let order = order.into_inner().unwrap();
        assert_eq!(order.len(), 6);
        // Dequeued in ascending order (the first started is the smallest).
        assert_eq!(order[0], 10);
        assert_eq!(gate.in_use(), (0, 0));
    }

    #[test]
    fn scheduler_survives_a_panicking_job() {
        let gate = CookGate::new(GIB);
        let done = AtomicUsize::new(0);
        let jobs: Vec<(JobCost, u32)> = (0..4)
            .map(|n| {
                (
                    JobCost {
                        peak_bytes: MIB,
                        heavy: false,
                    },
                    n,
                )
            })
            .collect();
        run_jobs(
            jobs,
            &gate,
            2,
            &|n| {
                if n == 1 {
                    panic!("cook failed");
                }
                done.fetch_add(1, Ordering::SeqCst);
            },
            &mut || {},
        );
        assert_eq!(done.load(Ordering::SeqCst), 3);
        assert_eq!(gate.in_use(), (0, 0));
    }

    #[test]
    fn eta_needs_a_finished_asset() {
        assert_eq!(eta_seconds(0, 100, 10.0), None);
        assert_eq!(eta_seconds(25, 100, 10.0), Some(30.0));
        assert_eq!(eta_seconds(100, 100, 10.0), Some(0.0));
        assert_eq!(format_duration(75.0), "1:15");
        assert_eq!(format_duration(3725.0), "1h02m");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vg-cook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_replaces_whole_files_and_leaves_no_temp() {
        let dir = scratch("atomic");
        let path = dir.join("a.vgeo");
        write_atomic(&path, b"old contents").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "tmp"))
            .collect();
        assert!(leftovers.is_empty());
        // A failed write (target is a directory) cleans its temp file up too
        // and never touches the target.
        let blocked = dir.join("dir.vgeo");
        std::fs::create_dir(&blocked).unwrap();
        assert!(write_atomic(&blocked, b"x").is_err());
        assert!(blocked.is_dir());
        assert!(std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|e| e.path().extension().is_none_or(|x| x != "tmp")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn killed_cook_leaves_no_valid_cache() {
        // A run killed mid-write leaves only a temp file: never a cache hit,
        // and it is not mistaken for one.
        let dir = scratch("killed");
        std::fs::write(dir.join("0123.vgeo.99-0.tmp"), b"partial").unwrap();
        assert!(read_vgeo(&dir, 0x0123).is_none());
        // Truncated or foreign `.vgeo` bytes are a miss, not a panic.
        std::fs::write(vgeo_path(&dir, 7), b"not a vgeo").unwrap();
        assert!(read_vgeo(&dir, 7).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn grid(n: usize) -> (Vec<PackedVertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                let mut v = PackedVertex::default();
                v.position = [x as f32, y as f32, ((x * y) % 7) as f32 * 0.05];
                vertices.push(v);
            }
        }
        let mut indices = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let i = (y * (n + 1) + x) as u32;
                let w = (n + 1) as u32;
                indices.extend([i, i + 1, i + w, i + 1, i + w + 1, i + w]);
            }
        }
        (vertices, indices)
    }

    #[test]
    fn cook_round_trips_and_second_call_is_a_cache_hit() {
        let dir = scratch("cook");
        let (vertices, indices) = grid(40);
        let content = stable_content_id(&vertices, &indices);
        let (_, hit) = load_or_build_vgeo_cache(content, &vertices, &indices, &dir).unwrap();
        assert!(!hit);
        assert!(read_vgeo(&dir, content).is_some());
        let (_, hit) = load_or_build_vgeo_cache(content, &vertices, &indices, &dir).unwrap();
        assert!(hit);
        // A different content id is its own cache entry.
        assert!(read_vgeo(&dir, content ^ 1).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn force_recook_ignores_a_valid_cache_until_this_run_cooks_it() {
        let dir = scratch("force");
        let (vertices, indices) = grid(40);
        let content = stable_content_id(&vertices, &indices);
        load_or_build_vgeo_cache(content, &vertices, &indices, &dir).unwrap();
        // As if a new run: nothing cooked yet.
        recooked().lock().unwrap().remove(&content);
        assert!(
            read_vgeo_with(&dir, content, false).is_some(),
            "valid cache is used normally"
        );
        assert!(
            read_vgeo_with(&dir, content, true).is_none(),
            "forced: the valid cache is ignored"
        );
        // Cooking again writes it and marks it done; it is read back normally afterwards.
        let before = std::fs::metadata(vgeo_path(&dir, content))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (_, hit) = {
            // The forced run misses the cache, so the build path runs and rewrites the file.
            let _ = read_vgeo_with(&dir, content, true);
            let bytes_before = std::fs::read(vgeo_path(&dir, content)).unwrap();
            std::fs::remove_file(vgeo_path(&dir, content)).unwrap();
            let r = load_or_build_vgeo_cache(content, &vertices, &indices, &dir).unwrap();
            assert_eq!(
                std::fs::read(vgeo_path(&dir, content)).unwrap(),
                bytes_before,
                "recook is deterministic"
            );
            r
        };
        assert!(!hit);
        assert!(
            std::fs::metadata(vgeo_path(&dir, content))
                .unwrap()
                .modified()
                .unwrap()
                >= before
        );
        assert!(
            read_vgeo_with(&dir, content, true).is_some(),
            "cooked this run: read back normally"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn placeholder_is_degenerate_and_distinct() {
        let a = placeholder_mesh(1);
        let b = placeholder_mesh(2);
        assert_eq!(a.indices.len(), 3);
        assert!(a.vertices.iter().all(|v| v.position == [0.0; 3]));
        assert_ne!(
            bytemuck::cast_slice::<_, u8>(&a.vertices),
            bytemuck::cast_slice::<_, u8>(&b.vertices)
        );
    }
}
