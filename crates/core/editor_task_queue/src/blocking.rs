//! Blocking calls on the UI thread, reported as tasks.
//!
//! The UI thread builds and updates views; it never waits on the disk, the
//! network, a process or a lock another thread holds. Functions that block
//! (`engine_fs::virtual_fs`, the file index, process spawning) call
//! [`blocking_call`] first. On the UI thread it records the call site:
//!
//! * each site once as a failed task, `Blocking call on the UI thread:
//!   virtual_fs::read_file`, whose log names the caller (`file:line`) and what
//!   the UI thread was running ([`crate::ui_watch`]); the site's count shows
//!   in the next report, at most one per site every [`REPORT_EVERY`];
//! * every call in [`blocking_snapshots`], busiest site first.
//!
//! With `PULSAR_STRICT_UI_THREAD=1` it panics instead, which is how tests and
//! debug sessions make a new UI-thread disk read fail where it is written.
//! [`allow_blocking`] is the escape hatch for the few calls that must block
//! (a modal native dialog): scoped and named.
//!
//! The fix for a reported site is to do the work in a task and apply its
//! result on the UI thread (`ui_common::background`).

use std::cell::Cell;
use std::collections::HashMap;
use std::panic::Location;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;

use crate::{global, now_ms, threads, TaskDescription};

/// Least time between two reports of one call site.
pub const REPORT_EVERY: Duration = Duration::from_secs(60);

/// One call site's tally.
#[derive(Clone, Debug)]
pub struct BlockingSnapshot {
    /// What blocked (`virtual_fs::read_file`).
    pub what: &'static str,
    /// Where it was called from.
    pub location: &'static Location<'static>,
    pub count: u64,
}

struct Site {
    what: &'static str,
    location: &'static Location<'static>,
    count: AtomicU64,
    /// When it was last reported as a task, 0 for never.
    reported_ms: AtomicU64,
}

type Key = (&'static str, &'static str, u32, u32);

fn sites() -> &'static Mutex<HashMap<Key, Arc<Site>>> {
    static SITES: OnceLock<Mutex<HashMap<Key, Arc<Site>>>> = OnceLock::new();
    SITES.get_or_init(Default::default)
}

fn strict() -> bool {
    static STRICT: OnceLock<bool> = OnceLock::new();
    *STRICT.get_or_init(|| std::env::var("PULSAR_STRICT_UI_THREAD").is_ok_and(|value| value == "1"))
}

thread_local! {
    /// Set inside [`allow_blocking`].
    static ALLOWED: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// Note that the caller is about to block on `what`. Off the UI thread this
/// is one thread-local read and nothing else.
#[track_caller]
pub fn blocking_call(what: &'static str) {
    if !threads::on_ui_thread() || ALLOWED.with(Cell::get).is_some() {
        return;
    }
    let location = Location::caller();
    if strict() {
        panic!(
            "{what} blocks, and was called on the UI thread at {location}; \
             do it in a task (.agents/EDITOR_TASKS.md, \"The UI thread never blocks\")"
        );
    }
    record(what, location);
}

/// Run `work` on this thread with blocking calls allowed, for the few that
/// must block the UI (a modal native dialog). `reason` says why, for review.
pub fn allow_blocking<R>(reason: &'static str, work: impl FnOnce() -> R) -> R {
    let previous = ALLOWED.with(|allowed| allowed.replace(Some(reason)));
    struct Restore(Option<&'static str>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ALLOWED.with(|allowed| allowed.set(self.0));
        }
    }
    let _restore = Restore(previous);
    work()
}

/// Every call site that blocked the UI thread, busiest first.
pub fn blocking_snapshots() -> Vec<BlockingSnapshot> {
    let mut all: Vec<_> = sites()
        .lock()
        .values()
        .map(|site| BlockingSnapshot {
            what: site.what,
            location: site.location,
            count: site.count.load(Ordering::Relaxed),
        })
        .collect();
    all.sort_by_key(|a| std::cmp::Reverse(a.count));
    all
}

fn record(what: &'static str, location: &'static Location<'static>) {
    let key = (what, location.file(), location.line(), location.column());
    let site = Arc::clone(sites().lock().entry(key).or_insert_with(|| {
        Arc::new(Site {
            what,
            location,
            count: AtomicU64::new(0),
            reported_ms: AtomicU64::new(0),
        })
    }));
    let count = site.count.fetch_add(1, Ordering::Relaxed) + 1;
    let now = now_ms();
    let last = site.reported_ms.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < REPORT_EVERY.as_millis() as u64 {
        return;
    }
    if site
        .reported_ms
        .compare_exchange(last, now.max(1), Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let running = crate::ui_watch::ui_thread_activity();
    let _ = global().scope(
        TaskDescription::foreground(format!("Blocking call on the UI thread: {what}"), "ui"),
        move |ctx| {
            ctx.error(format!(
                "{what} was called on the UI thread at {location} ({count} time(s) so far). \
                 The UI thread never blocks: do this in a task and apply the result on the UI \
                 thread (.agents/EDITOR_TASKS.md)."
            ));
            ctx.warn(format!("The UI thread was running: {running}"));
            Err::<(), String>(format!("{what} at {location}"))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::{register_current_thread, ThreadRole, ThreadSpec};

    fn count(what: &str) -> u64 {
        blocking_snapshots()
            .iter()
            .filter(|site| site.what == what)
            .map(|site| site.count)
            .sum()
    }

    #[test]
    fn blocking_on_the_ui_thread_is_counted_by_call_site() {
        std::thread::spawn(|| {
            let _ui = register_current_thread(ThreadSpec::new("test-ui", ThreadRole::Main));
            for _ in 0..3 {
                blocking_call("test::ui_read");
            }
            blocking_call("test::ui_read");
            let sites: Vec<_> = blocking_snapshots()
                .into_iter()
                .filter(|site| site.what == "test::ui_read")
                .collect();
            assert_eq!(sites.len(), 2, "two call sites");
            assert_eq!(count("test::ui_read"), 4);
            assert!(sites
                .iter()
                .all(|site| site.location.file().ends_with("blocking.rs")));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn other_threads_and_allowed_calls_are_not_counted() {
        std::thread::spawn(|| {
            let _worker = register_current_thread(ThreadSpec::new("test-io", ThreadRole::Io));
            blocking_call("test::worker_read");
        })
        .join()
        .unwrap();
        std::thread::spawn(|| {
            let _ui = register_current_thread(ThreadSpec::new("test-ui-2", ThreadRole::Main));
            allow_blocking("a modal dialog in a test", || {
                blocking_call("test::allowed")
            });
        })
        .join()
        .unwrap();
        assert_eq!(count("test::worker_read"), 0);
        assert_eq!(count("test::allowed"), 0);
    }
}
