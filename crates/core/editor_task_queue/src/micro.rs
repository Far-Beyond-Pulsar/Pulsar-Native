//! Micro tasks: work too short to deserve a task record.
//!
//! A task record, a span and a log buffer cost more than the work they would
//! describe when the work takes microseconds (a hit-test, a cache lookup, one
//! pass over a few hundred rows). [`micro`] keeps only per-name counters, and
//! turns a single run that overruns [`SLOW_THRESHOLD`] into a finished task so
//! the outlier is not lost in the average.

use crate::{global, TaskDescription};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// A micro run longer than this is also recorded as its own finished task.
pub const SLOW_THRESHOLD: Duration = Duration::from_millis(50);

#[derive(Default)]
struct Stat {
    count: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    slow: AtomicU64,
}

type Stats = RwLock<HashMap<&'static str, Arc<Stat>>>;

fn stats() -> &'static Stats {
    static STATS: OnceLock<Stats> = OnceLock::new();
    STATS.get_or_init(Default::default)
}

fn stat(name: &'static str) -> Arc<Stat> {
    if let Some(stat) = stats().read().get(name) {
        return Arc::clone(stat);
    }
    Arc::clone(stats().write().entry(name).or_default())
}

/// Run `work` and count it under `name`.
///
/// ```ignore
/// let hit = editor_task_queue::micro("viewport pick", || bvh.pick(ray));
/// ```
///
/// Costs two clock reads and a few atomics; safe in hot paths, not in the
/// innermost loop of a frame.
pub fn micro<R>(name: &'static str, work: impl FnOnce() -> R) -> R {
    let start = Instant::now();
    let value = work();
    let elapsed = start.elapsed();
    record(name, elapsed);
    value
}

fn record(name: &'static str, elapsed: Duration) {
    let stat = stat(name);
    let ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
    stat.count.fetch_add(1, Ordering::Relaxed);
    stat.total_ns.fetch_add(ns, Ordering::Relaxed);
    stat.max_ns.fetch_max(ns, Ordering::Relaxed);
    if elapsed >= SLOW_THRESHOLD {
        stat.slow.fetch_add(1, Ordering::Relaxed);
        record_slow_run(name, elapsed);
    }
}

fn record_slow_run(name: &'static str, elapsed: Duration) {
    global().scope(
        TaskDescription::foreground(format!("Slow micro task: {name}"), "micro"),
        |ctx| {
            ctx.warn(format!(
                "{name} took {:.1}ms; micro tasks are expected to finish in well under {}ms",
                elapsed.as_secs_f64() * 1000.0,
                SLOW_THRESHOLD.as_millis()
            ));
        },
    );
}

/// Aggregate of one micro task name.
#[derive(Clone, Debug)]
pub struct MicroSnapshot {
    pub name: &'static str,
    pub count: u64,
    pub total: Duration,
    pub max: Duration,
    /// Runs that overran [`SLOW_THRESHOLD`].
    pub slow: u64,
}

impl MicroSnapshot {
    pub fn mean(&self) -> Duration {
        if self.count == 0 {
            Duration::ZERO
        } else {
            self.total / self.count.min(u32::MAX as u64) as u32
        }
    }
}

/// Every micro task seen so far, busiest (by total time) first.
pub fn micro_snapshots() -> Vec<MicroSnapshot> {
    let mut snapshots: Vec<_> = stats()
        .read()
        .iter()
        .map(|(name, stat)| MicroSnapshot {
            name,
            count: stat.count.load(Ordering::Relaxed),
            total: Duration::from_nanos(stat.total_ns.load(Ordering::Relaxed)),
            max: Duration::from_nanos(stat.max_ns.load(Ordering::Relaxed)),
            slow: stat.slow.load(Ordering::Relaxed),
        })
        .collect();
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.total));
    snapshots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micro_counts_runs_and_returns_the_value() {
        let value = micro("test micro counts", || 41 + 1);
        micro("test micro counts", || ());
        assert_eq!(value, 42);
        let snapshot = micro_snapshots()
            .into_iter()
            .find(|s| s.name == "test micro counts")
            .unwrap();
        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.slow, 0);
        assert!(snapshot.max >= snapshot.mean());
    }

    #[test]
    fn a_slow_run_is_kept_as_its_own_task() {
        micro("test micro slow", || std::thread::sleep(SLOW_THRESHOLD * 2));
        let snapshot = micro_snapshots()
            .into_iter()
            .find(|s| s.name == "test micro slow")
            .unwrap();
        assert_eq!(snapshot.slow, 1);
        assert!(global()
            .snapshots()
            .iter()
            .any(|task| task.title == "Slow micro task: test micro slow"));
    }
}
