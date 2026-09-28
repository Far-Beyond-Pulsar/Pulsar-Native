//! Cross-process profiling: a profiled process publishes its instrumentation
//! events through shared memory, and a viewer in another process (the
//! editor's profiler) discovers it and pulls them without locks.
//!
//! # Layout
//!
//! Each profilable process maps one file, `<dir>/<pid>.pprof` (see
//! [`default_dir`]; `/dev/shm` on Linux, so it never touches a disk; the
//! temp directory on Windows and macOS, where the mapped pages stay in the
//! page cache):
//!
//! - a [`HEADER_SIZE`] header: magic, version, pid, kind, name, project,
//!   start time, a heartbeat, the control word the viewer writes, and the
//!   ring positions;
//! - a byte ring of `ring_capacity` bytes holding length-prefixed encoded
//!   [`ProfileEvent`]s.
//!
//! # Lock-free, end to end
//!
//! Instrumented threads never touch shared memory: `profile_scope!` pushes
//! onto the in-process lock-free queue exactly as before. One publisher
//! thread per target drains that queue into the ring (the single producer)
//! and the viewer reads it (the single consumer). Positions are monotonic
//! byte counters; the producer publishes with a release store of
//! `write_pos` after copying a record in, the consumer frees space with a
//! release store of `read_pos` after copying it out. A full ring drops the
//! record and counts it in `dropped`: the profiled process never waits.
//!
//! # Control
//!
//! Nothing is recorded until a viewer asks. The viewer claims the target
//! (`viewer_pid`), skips anything left in the ring and sets
//! [`CONTROL_RECORD`]; the publisher sees the change, clears and enables
//! the in-process profiler and starts streaming. Clearing the bit disables
//! it again. Until then the publisher only refreshes its heartbeat.
//!
//! # Platforms
//!
//! Windows, macOS and Linux alike: a file mapping (`memmap2`) shared by
//! both processes, plain atomics for the header and ring positions (both
//! processes run on the same machine, so the same layout and memory
//! model), and file listing for discovery. Only two things differ per
//! platform: the directory ([`default_dir`]) and how a target left behind
//! by a crashed process is recognised (`is_leftover`).
//!
//! # Opting in
//!
//! A game publishes only when started with [`ARG_FLAG`] or with
//! [`ENV_FLAG`] set ([`serve_if_requested`]); the editor launches games
//! with the variable set.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use memmap2::{Mmap, MmapMut};

use crate::events::ProfileEvent;
use crate::scope::init_profiler;

/// Environment variable that makes a process publish itself for profiling.
pub const ENV_FLAG: &str = "PULSAR_PROFILE";
/// Command-line flag with the same effect.
pub const ARG_FLAG: &str = "--pulsar-profile";
/// Overrides [`default_dir`].
pub const DIR_ENV: &str = "PULSAR_PROFILER_DIR";

const MAGIC: u64 = u64::from_le_bytes(*b"PLSRPROF");
const VERSION: u32 = 1;
/// Bytes reserved for the header; the ring starts here.
pub const HEADER_SIZE: usize = 4096;
/// Ring size of a target made by [`serve`]: at ~100 bytes an event, a few
/// hundred thousand events of slack between two viewer reads.
pub const DEFAULT_RING_BYTES: usize = 32 << 20;
/// How often the publisher refreshes its heartbeat while idle.
const IDLE_POLL: Duration = Duration::from_millis(100);
/// How often it drains the event queue while recording.
const RECORDING_POLL: Duration = Duration::from_millis(2);
/// A target whose heartbeat is older than this is listed as not responding.
pub const STALE_AFTER_MS: u64 = 2_000;

/// Viewer control bit: stream events.
pub const CONTROL_RECORD: u32 = 1;
/// Viewer control bit: lift the target's frame-rate cap while recording
/// (see [`crate::set_uncap_frame_rate`]).
pub const CONTROL_UNCAP: u32 = 2;

#[repr(C, align(64))]
struct Padded<T>(T);

/// The shared header. `repr(C)` and only atomics or fields written before
/// `magic` is published, so both processes agree on it.
#[repr(C)]
struct Header {
    /// Written last, with release ordering: a reader that sees it sees a
    /// fully initialised header.
    magic: AtomicU64,
    version: u32,
    pid: u32,
    started_unix_ms: u64,
    ring_capacity: u64,
    kind_len: u32,
    name_len: u32,
    project_len: u32,
    _reserved: u32,
    kind: [u8; 32],
    name: [u8; 128],
    project: [u8; 1024],
    /// Refreshed by the publisher; see [`STALE_AFTER_MS`].
    heartbeat_unix_ms: Padded<AtomicU64>,
    /// Written by the viewer: [`CONTROL_RECORD`], [`CONTROL_UNCAP`].
    control: AtomicU32,
    /// The viewer connection that owns the control word, 0 when none:
    /// its pid in the high 32 bits, a per-process connection number below.
    viewer: AtomicU64,
    /// Written by the publisher: 1 while it is streaming.
    recording: AtomicU32,
    /// Producer position (bytes ever written). Only the publisher stores.
    write_pos: Padded<AtomicU64>,
    /// Consumer position (bytes ever read). Only the viewer stores.
    read_pos: Padded<AtomicU64>,
    /// Records the publisher dropped because the ring was full.
    dropped: Padded<AtomicU64>,
}

const _: () = assert!(std::mem::size_of::<Header>() <= HEADER_SIZE);

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Where targets publish: [`DIR_ENV`] if set, else `/dev/shm/pulsar-profiler`
/// where that exists, else `<temp>/pulsar-profiler`.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        return PathBuf::from(dir);
    }
    let shm = Path::new("/dev/shm");
    if shm.is_dir() {
        return shm.join("pulsar-profiler");
    }
    std::env::temp_dir().join("pulsar-profiler")
}

/// Whether this process was asked to publish itself ([`ENV_FLAG`] set to
/// anything but `0`, or [`ARG_FLAG`] on the command line).
pub fn requested() -> bool {
    std::env::var(ENV_FLAG).is_ok_and(|v| v != "0" && !v.is_empty())
        || std::env::args().any(|arg| arg == ARG_FLAG)
}

fn copy_str(dst: &mut [u8], src: &str) -> u32 {
    // Truncate on a char boundary.
    let mut n = src.len().min(dst.len());
    while !src.is_char_boundary(n) {
        n -= 1;
    }
    dst[..n].copy_from_slice(&src.as_bytes()[..n]);
    n as u32
}

fn read_str(bytes: &[u8], len: u32) -> String {
    String::from_utf8_lossy(&bytes[..(len as usize).min(bytes.len())]).into_owned()
}

// ---- event encoding -------------------------------------------------------------

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_opt_str(out: &mut Vec<u8>, s: Option<&str>) {
    match s {
        None => put_u32(out, u32::MAX),
        Some(s) => {
            put_u32(out, s.len() as u32);
            out.extend_from_slice(s.as_bytes());
        }
    }
}

/// Encode `event` into `out` (cleared first).
pub fn encode_event(event: &ProfileEvent, out: &mut Vec<u8>) {
    out.clear();
    put_u64(out, event.scope_id);
    put_u64(out, event.parent_scope_id.map_or(0, |p| p.wrapping_add(1)));
    put_u64(out, event.thread_id);
    put_u32(out, event.process_id);
    put_u64(out, event.start_ns);
    put_u64(out, event.duration_ns);
    put_u32(out, event.depth);
    put_opt_str(out, Some(&event.name));
    put_opt_str(out, event.thread_name.as_deref());
    put_opt_str(out, event.parent_name.as_deref());
    put_opt_str(out, event.location.as_deref());
    put_opt_str(out, event.metadata.as_deref());
    put_opt_str(out, event.track_name.as_deref());
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn opt_str(&mut self) -> Option<Option<String>> {
        let len = self.u32()?;
        if len == u32::MAX {
            return Some(None);
        }
        Some(Some(String::from_utf8_lossy(self.take(len as usize)?).into_owned()))
    }
}

/// Decode one event encoded by [`encode_event`]. `None` if malformed.
pub fn decode_event(bytes: &[u8]) -> Option<ProfileEvent> {
    let mut r = Reader(bytes);
    let scope_id = r.u64()?;
    let parent = r.u64()?;
    let thread_id = r.u64()?;
    let process_id = r.u32()?;
    let start_ns = r.u64()?;
    let duration_ns = r.u64()?;
    let depth = r.u32()?;
    Some(ProfileEvent {
        scope_id,
        parent_scope_id: parent.checked_sub(1),
        thread_id,
        process_id,
        start_ns,
        duration_ns,
        depth,
        name: r.opt_str()??,
        thread_name: r.opt_str()?,
        parent_name: r.opt_str()?,
        location: r.opt_str()?,
        metadata: r.opt_str()?,
        track_name: r.opt_str()?,
    })
}

// ---- the ring --------------------------------------------------------------------

/// A mapped target file.
struct Region {
    ptr: *mut u8,
    ring_capacity: u64,
}

// SAFETY: the region is only accessed through atomics in the header and
// through the SPSC protocol for the ring (disjoint byte ranges per side).
unsafe impl Send for Region {}
unsafe impl Sync for Region {}

impl Region {
    fn header(&self) -> &Header {
        // SAFETY: the mapping is at least HEADER_SIZE bytes, page aligned.
        unsafe { &*(self.ptr as *const Header) }
    }

    fn ring(&self) -> *mut u8 {
        // SAFETY: the ring follows the header inside the mapping.
        unsafe { self.ptr.add(HEADER_SIZE) }
    }

    /// Copy `bytes` into the ring at byte position `pos`, wrapping.
    fn copy_in(&self, pos: u64, bytes: &[u8]) {
        let cap = self.ring_capacity as usize;
        let off = (pos % self.ring_capacity) as usize;
        let first = bytes.len().min(cap - off);
        // SAFETY: both ranges lie inside the ring; the producer owns
        // [write_pos, read_pos + capacity).
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ring().add(off), first);
            std::ptr::copy_nonoverlapping(bytes.as_ptr().add(first), self.ring(), bytes.len() - first);
        }
    }

    /// Copy `out.len()` bytes out of the ring from byte position `pos`.
    fn copy_out(&self, pos: u64, out: &mut [u8]) {
        let cap = self.ring_capacity as usize;
        let off = (pos % self.ring_capacity) as usize;
        let first = out.len().min(cap - off);
        // SAFETY: the consumer owns [read_pos, write_pos).
        unsafe {
            std::ptr::copy_nonoverlapping(self.ring().add(off), out.as_mut_ptr(), first);
            std::ptr::copy_nonoverlapping(self.ring(), out.as_mut_ptr().add(first), out.len() - first);
        }
    }

    /// Producer: append one record. `false` (and counted as dropped) when
    /// the ring has no room.
    fn push(&self, record: &[u8]) -> bool {
        let header = self.header();
        let needed = 4 + record.len() as u64;
        let write = header.write_pos.0.load(Ordering::Relaxed);
        let read = header.read_pos.0.load(Ordering::Acquire);
        if needed > self.ring_capacity / 4 || write - read + needed > self.ring_capacity {
            header.dropped.0.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.copy_in(write, &(record.len() as u32).to_le_bytes());
        self.copy_in(write + 4, record);
        header.write_pos.0.store(write + needed, Ordering::Release);
        true
    }
}

fn open_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

fn validate(map: &[u8]) -> io::Result<&Header> {
    if map.len() < HEADER_SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a profiling target"));
    }
    // SAFETY: the mapping holds at least a header; only atomics are read
    // before the magic check.
    let header = unsafe { &*(map.as_ptr() as *const Header) };
    if header.magic.load(Ordering::Acquire) != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a profiling target (yet)"));
    }
    if header.version != VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("profiling target version {}", header.version)));
    }
    if (map.len() as u64) < HEADER_SIZE as u64 + header.ring_capacity {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated profiling target"));
    }
    Ok(header)
}

// ---- the publishing side ---------------------------------------------------------

/// What a target says about itself.
#[derive(Clone, Debug)]
pub struct TargetDescription {
    /// `"editor"`, `"game"`, ...
    pub kind: String,
    pub name: String,
    pub project: String,
}

/// A running publisher. Dropping it stops the thread and removes the file.
pub struct Publisher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl Publisher {
    /// The file other processes discover this target by.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Publish this process for profiling in [`default_dir`].
pub fn serve(description: TargetDescription) -> io::Result<Publisher> {
    serve_in(&default_dir(), description, DEFAULT_RING_BYTES)
}

/// [`serve`] if [`requested`] (see [`publish_process`]). Returns whether
/// the process is published.
pub fn serve_if_requested(description: TargetDescription) -> bool {
    requested() && publish_process(description)
}

/// Publish this process in [`default_dir`] for the rest of its life (once;
/// later calls return the first outcome). Returns whether it is published.
pub fn publish_process(description: TargetDescription) -> bool {
    static PUBLISHER: std::sync::OnceLock<Option<Publisher>> = std::sync::OnceLock::new();
    PUBLISHER
        .get_or_init(|| match serve(description) {
            Ok(publisher) => Some(publisher),
            Err(error) => {
                eprintln!("pulsar profiler: could not publish this process: {error}");
                None
            }
        })
        .is_some()
}

/// Publish this process in `dir` with a ring of `ring_bytes`.
pub fn serve_in(dir: &Path, description: TargetDescription, ring_bytes: usize) -> io::Result<Publisher> {
    std::fs::create_dir_all(dir)?;
    let pid = std::process::id();
    let path = dir.join(format!("{pid}.pprof"));
    let ring_capacity = ring_bytes.max(4096) as u64;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path)?;
    file.set_len(HEADER_SIZE as u64 + ring_capacity)?;
    // SAFETY: the file is ours; other processes only map it, and every
    // shared access goes through the header atomics and the SPSC ring.
    let mut map = unsafe { MmapMut::map_mut(&file)? };
    {
        // SAFETY: freshly created, zero-filled, and not yet published (the
        // magic is still 0), so no reader looks at these fields yet.
        let header = unsafe { &mut *(map.as_mut_ptr() as *mut Header) };
        header.version = VERSION;
        header.pid = pid;
        header.started_unix_ms = now_unix_ms();
        header.ring_capacity = ring_capacity;
        header.kind_len = copy_str(&mut header.kind, &description.kind);
        header.name_len = copy_str(&mut header.name, &description.name);
        header.project_len = copy_str(&mut header.project, &description.project);
        header.heartbeat_unix_ms.0.store(now_unix_ms(), Ordering::Relaxed);
        header.magic.store(MAGIC, Ordering::Release);
    }
    let region = Region { ptr: map.as_mut_ptr(), ring_capacity };
    let stop = Arc::new(AtomicBool::new(false));
    let thread = std::thread::Builder::new()
        .name("pulsar-profiler-publisher".into())
        .spawn({
            let stop = Arc::clone(&stop);
            move || {
                // The mapping lives as long as the thread uses it.
                let _map = map;
                publish_loop(&region, &stop);
            }
        })?;
    Ok(Publisher { stop, thread: Some(thread), path })
}

fn publish_loop(region: &Region, stop: &AtomicBool) {
    let header = region.header();
    let profiler = init_profiler();
    let mut recording = false;
    let mut batch = Vec::new();
    let mut record = Vec::new();
    while !stop.load(Ordering::Acquire) {
        header.heartbeat_unix_ms.0.store(now_unix_ms(), Ordering::Relaxed);
        let control = header.control.load(Ordering::Acquire);
        let want = control & CONTROL_RECORD != 0;
        if want != recording {
            // Edge-triggered: a process that also profiles itself locally
            // (the editor) is only affected when a viewer starts or stops.
            if want {
                profiler.clear();
                profiler.enable();
            } else {
                profiler.disable();
            }
            crate::options::set_uncap_frame_rate(want && control & CONTROL_UNCAP != 0);
            recording = want;
            header.recording.store(u32::from(want), Ordering::Release);
        }
        if recording {
            profiler.drain_pending(&mut batch);
            for event in batch.drain(..) {
                encode_event(&event, &mut record);
                region.push(&record);
            }
        }
        std::thread::sleep(if recording { RECORDING_POLL } else { IDLE_POLL });
    }
    if recording {
        profiler.disable();
        crate::options::set_uncap_frame_rate(false);
    }
    header.recording.store(0, Ordering::Release);
}

// ---- the viewing side --------------------------------------------------------------

/// A profilable process found by [`list_targets`].
#[derive(Clone, Debug)]
pub struct TargetInfo {
    pub path: PathBuf,
    pub pid: u32,
    pub kind: String,
    pub name: String,
    pub project: String,
    pub started_unix_ms: u64,
    /// Milliseconds since the target's last heartbeat.
    pub heartbeat_age_ms: u64,
    /// The target is streaming to a viewer.
    pub recording: bool,
    /// The viewer that controls it, if any.
    pub viewer_pid: Option<u32>,
}

impl TargetInfo {
    /// The target's publisher is alive (recent heartbeat).
    pub fn responsive(&self) -> bool {
        self.heartbeat_age_ms <= STALE_AFTER_MS
    }

    /// This is the calling process.
    pub fn is_current_process(&self) -> bool {
        self.pid == std::process::id()
    }
}

fn describe(path: &Path) -> io::Result<TargetInfo> {
    let file = File::open(path)?;
    // SAFETY: read-only mapping; fields are read after the magic check.
    let map = unsafe { Mmap::map(&file)? };
    let header = validate(&map)?;
    let viewer = header.viewer.load(Ordering::Acquire);
    Ok(TargetInfo {
        path: path.to_owned(),
        pid: header.pid,
        kind: read_str(&header.kind, header.kind_len),
        name: read_str(&header.name, header.name_len),
        project: read_str(&header.project, header.project_len),
        started_unix_ms: header.started_unix_ms,
        heartbeat_age_ms: now_unix_ms().saturating_sub(header.heartbeat_unix_ms.0.load(Ordering::Relaxed)),
        recording: header.recording.load(Ordering::Acquire) != 0,
        viewer_pid: (viewer != 0).then_some((viewer >> 32) as u32),
    })
}

/// Whether process `pid` still exists (best effort; `true` when unknown).
fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks for existence and permission.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Whether a target that stopped heartbeating belongs to a process that
/// exited. Unix asks the OS about the pid. Windows cannot delete a file
/// some process still maps, so deleting it is the test: it only succeeds
/// for a leftover (the caller's own open connections aside).
fn is_leftover(info: &TargetInfo) -> bool {
    if cfg!(unix) {
        !process_alive(info.pid)
    } else {
        std::fs::remove_file(&info.path).is_ok()
    }
}

/// Every profilable process in [`default_dir`], newest first.
pub fn list_targets() -> Vec<TargetInfo> {
    list_targets_in(&default_dir())
}

/// Every profilable process in `dir`, newest first. Files left behind by
/// processes that exited are removed.
pub fn list_targets_in(dir: &Path) -> Vec<TargetInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut targets: Vec<TargetInfo> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "pprof"))
        .filter_map(|path| match describe(&path) {
            Ok(info) if !info.responsive() && is_leftover(&info) => {
                let _ = std::fs::remove_file(&path);
                None
            }
            Ok(info) => Some(info),
            Err(_) => {
                // Unreadable. On Windows a live target's file cannot be
                // deleted, so this only ever removes leftovers.
                if !cfg!(unix) {
                    let _ = std::fs::remove_file(&path);
                }
                None
            }
        })
        .collect();
    targets.sort_by(|a, b| b.started_unix_ms.cmp(&a.started_unix_ms));
    targets
}

/// A viewer's connection to one target.
pub struct TargetConnection {
    region: Region,
    _map: MmapMut,
    pid: u32,
    /// This connection's viewer token (see `Header::viewer`).
    token: u64,
    owns_control: bool,
    scratch: Vec<u8>,
}

fn next_viewer_token() -> u64 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    (u64::from(std::process::id()) << 32) | u64::from(NEXT.fetch_add(1, Ordering::Relaxed))
}

impl TargetConnection {
    /// Map the target at `path`.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = open_file(path)?;
        // SAFETY: shared mapping; see `Region`.
        let mut map = unsafe { MmapMut::map_mut(&file)? };
        let (pid, ring_capacity) = {
            let header = validate(&map)?;
            (header.pid, header.ring_capacity)
        };
        let region = Region { ptr: map.as_mut_ptr(), ring_capacity };
        Ok(Self { region, _map: map, pid, token: next_viewer_token(), owns_control: false, scratch: Vec::new() })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Milliseconds since the target's last heartbeat.
    pub fn heartbeat_age_ms(&self) -> u64 {
        now_unix_ms().saturating_sub(self.region.header().heartbeat_unix_ms.0.load(Ordering::Relaxed))
    }

    /// The target acknowledged recording (it is streaming).
    pub fn is_streaming(&self) -> bool {
        self.region.header().recording.load(Ordering::Acquire) != 0
    }

    /// Records the target dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.region.header().dropped.0.load(Ordering::Relaxed)
    }

    /// Claim the target and ask it to stream. Fails if another live viewer
    /// controls it.
    pub fn start_recording(&mut self, uncap_frame_rate: bool) -> Result<(), String> {
        let header = self.region.header();
        let mut owner = header.viewer.load(Ordering::Acquire);
        loop {
            // A viewer that died without releasing the target is taken over.
            if owner != 0 && owner != self.token && process_alive((owner >> 32) as u32) {
                return Err(format!("process {} is already recording this target", owner >> 32));
            }
            match header.viewer.compare_exchange(owner, self.token, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(current) => owner = current,
            }
        }
        self.owns_control = true;
        // Skip anything a previous session left in the ring.
        let write = header.write_pos.0.load(Ordering::Acquire);
        header.read_pos.0.store(write, Ordering::Release);
        let control = CONTROL_RECORD | if uncap_frame_rate { CONTROL_UNCAP } else { 0 };
        header.control.store(control, Ordering::Release);
        Ok(())
    }

    /// Ask the target to stop streaming and release it.
    pub fn stop_recording(&mut self) {
        if !self.owns_control {
            return;
        }
        let header = self.region.header();
        header.control.store(0, Ordering::Release);
        let _ = header.viewer.compare_exchange(self.token, 0, Ordering::AcqRel, Ordering::Acquire);
        self.owns_control = false;
    }

    /// Append every event published since the last call to `out`; returns
    /// how many.
    pub fn read_events(&mut self, out: &mut Vec<ProfileEvent>) -> usize {
        let header = self.region.header();
        let write = header.write_pos.0.load(Ordering::Acquire);
        let mut read = header.read_pos.0.load(Ordering::Relaxed);
        let before = out.len();
        while write - read >= 4 {
            let mut len = [0u8; 4];
            self.region.copy_out(read, &mut len);
            let len = u32::from_le_bytes(len) as u64;
            if write - read < 4 + len {
                break;
            }
            self.scratch.resize(len as usize, 0);
            self.region.copy_out(read + 4, &mut self.scratch);
            if let Some(event) = decode_event(&self.scratch) {
                out.push(event);
            }
            read += 4 + len;
        }
        header.read_pos.0.store(read, Ordering::Release);
        out.len() - before
    }
}

impl Drop for TargetConnection {
    fn drop(&mut self) {
        self.stop_recording();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(i: u64) -> ProfileEvent {
        ProfileEvent {
            scope_id: i,
            parent_scope_id: (i % 2 == 0).then_some(i / 2),
            name: format!("scope-{i}"),
            thread_id: 7,
            thread_name: Some("worker ✓".into()),
            process_id: 42,
            parent_name: None,
            start_ns: 1_000 + i,
            duration_ns: 10 * i,
            depth: (i % 5) as u32,
            location: Some("file.rs:1".into()),
            metadata: None,
            track_name: Some("gpu".into()),
        }
    }

    #[test]
    fn events_round_trip() {
        let mut buf = Vec::new();
        for i in 0..10 {
            let e = event(i);
            encode_event(&e, &mut buf);
            let d = decode_event(&buf).unwrap();
            assert_eq!(
                (d.scope_id, d.parent_scope_id, d.name, d.thread_name, d.start_ns, d.duration_ns, d.depth, d.location, d.metadata, d.track_name),
                (e.scope_id, e.parent_scope_id, e.name, e.thread_name, e.start_ns, e.duration_ns, e.depth, e.location, e.metadata, e.track_name)
            );
        }
        assert!(decode_event(&buf[..buf.len() - 1]).is_none(), "truncated records are rejected");
    }

    #[test]
    fn the_ring_wraps_and_drops_instead_of_blocking() {
        let dir = tempfile_dir("ring");
        // A second mapping of the same file stands in for another process.
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(dir.join("r.pprof")).unwrap();
        let cap = 4096u64;
        file.set_len(HEADER_SIZE as u64 + cap).unwrap();
        let mut producer_map = unsafe { MmapMut::map_mut(&file).unwrap() };
        let producer = Region { ptr: producer_map.as_mut_ptr(), ring_capacity: cap };
        unsafe { &mut *(producer_map.as_mut_ptr() as *mut Header) }.ring_capacity = cap;
        let mut consumer_map = unsafe { MmapMut::map_mut(&file).unwrap() };
        let consumer = TargetConnection {
            region: Region { ptr: consumer_map.as_mut_ptr(), ring_capacity: cap },
            _map: unsafe { MmapMut::map_mut(&file).unwrap() },
            pid: 0,
            token: 0,
            owns_control: false,
            scratch: Vec::new(),
        };
        let mut consumer = consumer;
        let mut buf = Vec::new();
        let mut next = 0u64;
        let mut seen = Vec::new();
        // Many laps around a 4 KiB ring.
        for _ in 0..200 {
            for _ in 0..7 {
                encode_event(&event(next), &mut buf);
                assert!(producer.push(&buf));
                next += 1;
            }
            consumer.read_events(&mut seen);
        }
        assert_eq!(seen.len() as u64, next);
        assert!(seen.iter().enumerate().all(|(i, e)| e.scope_id == i as u64 && e.name == format!("scope-{i}")));
        // Fill without reading: pushes fail and are counted, never block.
        let mut refused = 0;
        for i in 0..1000 {
            encode_event(&event(i), &mut buf);
            if !producer.push(&buf) {
                refused += 1;
            }
        }
        assert!(refused > 900);
        assert_eq!(consumer.dropped(), refused);
        drop(consumer_map);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn tempfile_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulsar-profiler-unit-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
