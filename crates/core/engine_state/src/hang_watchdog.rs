//! # Hang Watchdog
//!
//! Detects when a helio-engine activity loop silently freezes while the
//! process as a whole stays alive — the classic symptom when entering the
//! level editor "a few frames in", or when the Flame Graph profiler window
//! opens before any record/profile is even shown.
//!
//! ## Why this exists
//!
//! The two "data collection" systems in the app both funnel through the
//! shared `pulsar-profiling` global (de-forked, git+feature checkout):
//!
//!   1. **HelioRenderer frame profiling** — per-frame
//!      `enable_profiling / clear_events / collect_events / disable_profiling`
//!      in the helio renderer frame path.
//!   2. **`ui_flamegraph::InstrumentationCollector`** — a dedicated collector
//!      thread draining profiling events into the flamegraph database.
//!
//! Our static analysis ruled the shared profiling crate itself out as the
//! deadlock source (unbounded channel, parking_lot RwLock only, no Condvar,
//! no blocking recv — verified). The freeze therefore lives at a *different
//! lock* shared between the level-editor frame path and the window-open path.
//! Static inspection can't name the exact pair, so this module ships a
//! **runtime watchdog** that, on a freeze, dumps exactly which thread is
//! blocked on which lock — automatically, every time, with zero manual
//! steps.
//!
//! ## How the watchdog works
//!
//! * A **watchdog OS thread** is spawned once (from `main`, before any
//!   window/GPU work). It owns **no project locks** — it is free to run at any
//!   moment precisely because the hang victims are *not* it.
//! * Every activity loop calls `watchdog::heartbeat()` / `component_heartbeat`
//!   on each iteration — cheap (`AtomicU64` store + a tiny lock-free ring that
//!   records *which named subsystem* made progress most recently).
//! * If no heartbeat arrives for `FREEZE_GRACE`:
//!     1. We call **`parking_lot::deadlock::check_for_deadlocks()`**. With the
//!        workspace `deadlock_detection` feature now enabled, parking_lot has
//!        been building the global wait-for graph for every lock we touch, so
//!        this one call prints the **exact lock pair + each blocked thread +
//!        that thread's name + a captured backtrace** for the real cycle.
//!     2. We dump the last N named heartbeats (the "last things that were
//!        making progress") to a hang-report log file so even when the cycle
//!        isn't a pure mutex-mutex cycle the report shows the *direction* —
//!        which subsystem was alive right before the freeze.
//!     3. We emit a `tracing::error!` pointing at the report path — no panic,
//!        the app keeps limping so you still have the session if it recovers.
//!
//! Enabled via the workspace `pulsar_watchdog` / `deadlock_detection` wiring.
//! The watchdog itself intentionally uses no `parking_lot` lock that any of
//! the instrumented systems could ever contend on — heartbeat writes go
//! through `AtomicU64`/`SeqCst`-free ring slots, never a `Mutex`.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

/// Also lets the app ask "are we currently frozen?" for the top-level error.
static FROZEN: AtomicBool = AtomicBool::new(false);

// ---------------- monotonic clock ----------------
static EPOCH: Once = Once::new();
static EPOCH_NOW: OnceLock<Instant> = OnceLock::new();

fn epoch_now() -> Instant {
    EPOCH_NOW.get_or_init(|| {
        EPOCH.call_once(|| {});
        Instant::now()
    })
}

/// ms since process start (monotonic, never goes backwards).
fn now_ms() -> u64 {
    epoch_now().elapsed().as_millis() as u64
}

// ---------------- heartbeat meat ----------------
/// Set by every activity-loop heartbeat. 0 = never.
static LAST_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0 Patty);

/// Freeze detection grace.
pub const FREEZE_GRACE: Duration = Duration::from_secs(3);

/// Ring of last `RING` named beats — printed on freeze so you see which
/// subsystem was the last one alive.
const RING: usize = 64;
static RING_NAMES: OnceLock<Vec<AtomicMoll>> = OnceLock::new();
static RING_TIME_MS: OnceLock<Vec<AtomicU64>> = OnceLock::new();

/// Called by the level-editor / engine frame loop and the flamegraph
/// collector loop, once per iteration. Cheap, allocation-free.
pub fn heartbeat() {
    LAST_HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
    FROZEN.store(false, Ordering::Relaxed);
}

/// `heartbeat()` + record which named subsystem is currently alive; the
/// trap report will show the last few names so the freeze direction is
/// obvious even when no parking_lot cycle exists.
pub fn component_heartbeat(name: &'static str) {
    heartbeat();
    let names = RING_NAMES.get_or_init(|| (0..RING).map(|_| AtomicMoll(none)).collect());
    let times = RING_TIME_MS.get_or_init(|| (0..RING).map(|_| AtomicU64::new(0)).collect());
    // 1. bump the "epoch counter" under `EPOCH`'s first call via heartbeat().
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let idx = (SEQ.fetch_add(1, Ordering::Relaxed) as usize) % RING;
    names[idx].set_name(name);
    times[idx].store(now_ms(), Ordering::Relaxed);
}

// ---------------- report dumping ----------------
static REPORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static REPORTED_ONCE: AtomicBool = AtomicBool::new(false);
static REPORT_DIR: Mutexnable<(String)>... 
