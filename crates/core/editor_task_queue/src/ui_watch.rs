//! The UI thread's responsiveness, reported as tasks.
//!
//! The UI thread beats ([`ui_beat`]) from a timer on its own executor, so a
//! beat only happens when the thread is free to run it. A watcher thread
//! samples the beat; when it stops for longer than [`UNRESPONSIVE_AFTER`] the
//! watcher samples what the UI thread is running (the editor task polled on
//! it, its phase) until the beats resume, then records the episode as a
//! failed task, `UI thread unresponsive for 3.2s`, whose log says what was
//! seen. Work that held the UI thread without a task is named as such: it is
//! the work to move into a task.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Once;
use std::time::Duration;

use crate::thread_stack;
use crate::threads::{self, ThreadRole, ThreadSpec};
use crate::{global, now_ms, TaskDescription};

/// Longest the UI thread may go without a beat before it counts as blocked.
pub const UNRESPONSIVE_AFTER: Duration = Duration::from_millis(500);
/// How often the UI thread's executor should call [`ui_beat`].
pub const BEAT_EVERY: Duration = Duration::from_millis(100);
/// How often the watcher samples.
const SAMPLE_EVERY: Duration = Duration::from_millis(50);
/// A gap in the watcher's own sampling this long means the whole process was
/// suspended (a debugger, the machine sleeping), not that the UI was blocked.
const SUSPENDED_AFTER: Duration = Duration::from_secs(2);

static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);
/// The OS id of the thread that beats (the UI thread), for its stack.
static UI_THREAD_ID: AtomicU32 = AtomicU32::new(0);
/// A stall this long gets its stack logged while it is still going, so a
/// UI thread that never comes back is still explained (the episode is only
/// recorded as a task when the beats resume).
const STALL_STACK_AFTER: Duration = Duration::from_secs(5);
/// How often the stack of a stall that goes on is logged again.
const STALL_STACK_EVERY: Duration = Duration::from_secs(30);

/// The UI framework's own account of what the UI thread is running (the
/// callback, view or foreground future, as a path outer to inner). The
/// framework cannot depend on this crate, so the engine installs it.
static UI_ACTIVITY_SOURCE: std::sync::OnceLock<fn() -> Option<String>> = std::sync::OnceLock::new();

/// Install the UI framework's account of what the UI thread is running.
/// Called once by the engine at startup.
pub fn set_ui_activity_source(source: fn() -> Option<String>) {
    let _ = UI_ACTIVITY_SOURCE.set(source);
}

/// Record UI work that ran longer than the framework's budget: one failed
/// task per `what`, at most once per minute each (with how many were held
/// back). The UI framework reports these through a hook the engine installs.
pub fn record_slow_ui_work(what: String, elapsed: Duration) {
    use parking_lot::Mutex;
    use std::collections::HashMap;
    static LAST: std::sync::OnceLock<Mutex<HashMap<String, (u64, u32)>>> =
        std::sync::OnceLock::new();
    let now = now_ms();
    let held_back = {
        let mut last = LAST.get_or_init(Default::default).lock();
        let entry = last.entry(what.clone()).or_insert((0, 0));
        if entry.0 != 0 && now.saturating_sub(entry.0) < 60_000 {
            entry.1 += 1;
            return;
        }
        let held_back = entry.1;
        *entry = (now, 0);
        held_back
    };
    let title = format!(
        "Slow UI work ({:.0}ms): {what}",
        elapsed.as_secs_f64() * 1000.0
    );
    let _ = global().scope(
        TaskDescription::foreground(title.clone(), "ui"),
        move |ctx| {
            ctx.error(format!(
                "{what} held the UI thread for {:.1}ms. The UI thread builds and updates views; \
             anything else runs in a task that sends its result back (.agents/EDITOR_TASKS.md).",
                elapsed.as_secs_f64() * 1000.0
            ));
            if held_back > 0 {
                ctx.warn(format!(
                "{held_back} more slow run(s) of the same work in the last minute were not reported"
            ));
            }
            Err::<(), String>(title)
        },
    );
}

/// Called by the UI thread every [`BEAT_EVERY`], from a timer on its own
/// executor. The first call starts the watcher.
pub fn ui_beat() {
    LAST_BEAT_MS.store(now_ms(), Ordering::Release);
    if UI_THREAD_ID.load(Ordering::Relaxed) == 0 {
        UI_THREAD_ID.store(thread_stack::current_thread_id(), Ordering::Relaxed);
    }
    static START: Once = Once::new();
    START.call_once(start_watcher);
}

fn start_watcher() {
    let spawned = threads::spawn_thread(
        ThreadSpec::new("UI responsiveness watcher", ThreadRole::Other).owner("editor_task_queue"),
        || {
            let mut episode: Option<Episode> = None;
            let mut last_sample = now_ms();
            loop {
                std::thread::sleep(SAMPLE_EVERY);
                let now = now_ms();
                let suspended =
                    now.saturating_sub(last_sample) >= SUSPENDED_AFTER.as_millis() as u64;
                last_sample = now;
                let beat = LAST_BEAT_MS.load(Ordering::Acquire);
                if suspended {
                    episode = None;
                    continue;
                }
                let silent = now.saturating_sub(beat);
                if silent >= UNRESPONSIVE_AFTER.as_millis() as u64 {
                    let episode = episode.get_or_insert_with(|| Episode::new(beat));
                    episode.sample();
                    episode.report_ongoing(silent);
                } else if let Some(ended) = episode.take() {
                    ended.record(beat);
                }
            }
        },
    );
    if let Err(error) = spawned {
        tracing::error!("the UI responsiveness watcher did not start: {error}");
    }
}

/// One stretch with no beats: what the UI thread was seen running.
struct Episode {
    /// The last beat before it.
    from_ms: u64,
    /// What the UI thread was doing, by how many samples saw it.
    seen: BTreeMap<String, u32>,
    samples: u32,
    /// When the stack of this still-running stall was last logged.
    stack_logged_ms: Option<u64>,
}

impl Episode {
    fn new(from_ms: u64) -> Self {
        Self {
            from_ms,
            seen: BTreeMap::new(),
            samples: 0,
            stack_logged_ms: None,
        }
    }

    /// A stall that is still going after [`STALL_STACK_AFTER`] logs the UI
    /// thread's stack (and again every [`STALL_STACK_EVERY`]): a thread that
    /// never comes back never ends its episode, so nothing else would say
    /// what it waits on. Logged as an error the log file keeps, with the
    /// frame where the wait is (a lock's `lock_shared_slow`, a channel's
    /// `recv`, a GPU wait) and the callers that asked for it.
    fn report_ongoing(&mut self, silent_ms: u64) {
        if silent_ms < STALL_STACK_AFTER.as_millis() as u64 {
            return;
        }
        let now = now_ms();
        if self
            .stack_logged_ms
            .is_some_and(|at| now.saturating_sub(at) < STALL_STACK_EVERY.as_millis() as u64)
        {
            return;
        }
        self.stack_logged_ms = Some(now);
        let id = UI_THREAD_ID.load(Ordering::Relaxed);
        let stack = (id != 0)
            .then(|| thread_stack::capture(id))
            .flatten()
            .map(|frames| frames.join("\n    "))
            .unwrap_or_else(|| "(the stack is not available on this platform)".to_owned());
        tracing::error!(
            "[UI-STALL] the UI thread has run no frame for {:.1}s; it is {}. Its stack:\n    {stack}",
            silent_ms as f64 / 1000.0,
            ui_thread_activity(),
        );
    }

    fn sample(&mut self) {
        self.samples += 1;
        *self.seen.entry(ui_thread_activity()).or_default() += 1;
    }

    /// Record the episode, which ended with the beat at `to_ms`.
    fn record(self, to_ms: u64) {
        // The beat after the gap is up to one period late by design.
        let blocked = Duration::from_millis(
            to_ms
                .saturating_sub(self.from_ms)
                .saturating_sub(BEAT_EVERY.as_millis() as u64),
        );
        let title = format!("UI thread unresponsive for {:.1}s", blocked.as_secs_f64());
        let mut seen: Vec<_> = self.seen.into_iter().collect();
        seen.sort_by_key(|a| std::cmp::Reverse(a.1));
        let summary = seen
            .first()
            .map(|(what, _)| what.clone())
            .unwrap_or_else(|| "nothing sampled".to_owned());
        let samples = self.samples.max(1);
        let _ = global().scope(TaskDescription::foreground(title.clone(), "ui"), move |ctx| {
            ctx.error(format!(
                "The UI thread ran no frame for {:.0}ms (more than {}ms). Work that long belongs in an editor task (.agents/EDITOR_TASKS.md).",
                blocked.as_secs_f64() * 1000.0,
                UNRESPONSIVE_AFTER.as_millis()
            ));
            for (what, count) in &seen {
                ctx.warn(format!(
                    "{}% of samples: {what}",
                    count * 100 / samples
                ));
            }
            Err::<(), String>(format!("{title}: {summary}"))
        });
    }
}

/// What the UI thread is running right now, as one line.
pub(crate) fn ui_thread_activity() -> String {
    let task = ui_thread_task();
    match UI_ACTIVITY_SOURCE.get().and_then(|source| source()) {
        Some(framework) => format!("{framework}; {task}"),
        None => task,
    }
}

fn ui_thread_task() -> String {
    let Some(ui) = threads::thread_snapshots()
        .into_iter()
        .find(|thread| thread.role == ThreadRole::Main)
    else {
        return "the UI thread is not registered".to_owned();
    };
    let phase = ui
        .phase
        .map(|phase| format!(" (phase: {phase})"))
        .unwrap_or_default();
    match ui.task {
        Some(id) => {
            let title = global()
                .snapshots()
                .into_iter()
                .find(|task| task.id == id)
                .map(|task| {
                    let phase = task
                        .phase
                        .map(|phase| format!(", phase \"{phase}\""))
                        .unwrap_or_default();
                    format!("task \"{}\" from {}{phase}", task.title, task.origin)
                })
                .unwrap_or_else(|| format!("task {}", id.0));
            format!("{title}{phase}")
        }
        None => format!("untracked work, no editor task on the UI thread{phase}"),
    }
}
