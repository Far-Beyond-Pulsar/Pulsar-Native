//! The engine's live log: the one store of recent log lines.
//!
//! The engine's logging layer publishes every event here ([`publish_live_log`],
//! [`publish_live_log_in_task`]). The log drawer subscribes to it
//! ([`subscribe_live_logs`]) and the MCP log tools read it
//! ([`read_since`], [`wait_for_new`]): one buffer, two readers, so what an AI
//! client queries is what the person sees.
//!
//! Every entry has a sequence number that only grows, so a reader keeps a cursor
//! (the number after the last entry it saw) and asks for what came after; lines that
//! fell out of the ring before it asked are counted, not lost silently.

use once_cell::sync::Lazy;
use parking_lot::{Condvar, Mutex};
use smol::channel::{self, Receiver, Sender, TrySendError};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const LIVE_LOG_BUFFER: usize = 20_000;
const LIVE_LOG_CHANNEL_CAPACITY: usize = 2048;

/// One published line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveEntry {
    /// Grows by one per entry, from 0.
    pub seq: u64,
    /// `YYYY-MM-DD HH:MM:SS.mmm LEVEL target: message`.
    pub line: String,
    /// The editor task the event was emitted in, when it was in one.
    pub task: Option<u64>,
}

struct LiveLogBus {
    subscribers: Vec<Sender<String>>,
    entries: VecDeque<LiveEntry>,
    /// Sequence number of the next entry; the first in `entries` is
    /// `next_seq - entries.len()`.
    next_seq: u64,
}

impl LiveLogBus {
    fn new() -> Self {
        Self {
            subscribers: Vec::new(),
            entries: VecDeque::with_capacity(4096),
            next_seq: 0,
        }
    }

    fn first_seq(&self) -> u64 {
        self.next_seq - self.entries.len() as u64
    }
}

static LIVE_LOG_BUS: Lazy<Mutex<LiveLogBus>> = Lazy::new(|| Mutex::new(LiveLogBus::new()));
/// Woken by every publish, for [`wait_for_new`].
static PUBLISHED: Condvar = Condvar::new();

pub fn publish_live_log(line: impl Into<String>) {
    publish_live_log_in_task(line, None);
}

/// Publish a line emitted in the editor task `task` (see
/// `editor_task_queue::task_of_event`).
pub fn publish_live_log_in_task(line: impl Into<String>, task: Option<u64>) {
    let line = line.into();
    let mut bus = LIVE_LOG_BUS.lock();

    if bus.entries.len() >= LIVE_LOG_BUFFER {
        bus.entries.pop_front();
    }
    let seq = bus.next_seq;
    bus.next_seq += 1;
    bus.entries.push_back(LiveEntry {
        seq,
        line: line.clone(),
        task,
    });

    bus.subscribers
        .retain(|tx| match tx.try_send(line.clone()) {
            Ok(_) => true,
            Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Closed(_)) => false,
        });
    drop(bus);
    PUBLISHED.notify_all();
}

pub fn subscribe_live_logs() -> Receiver<String> {
    let (tx, rx) = channel::bounded(LIVE_LOG_CHANNEL_CAPACITY);
    let mut bus = LIVE_LOG_BUS.lock();

    // The drawer's own buffer is the most recent few thousand lines.
    let skip = bus.entries.len().saturating_sub(4000);
    for entry in bus.entries.iter().skip(skip) {
        if tx.try_send(entry.line.clone()).is_err() {
            break;
        }
    }

    bus.subscribers.push(tx);
    rx
}

/// What a read returns.
#[derive(Clone, Debug, Default)]
pub struct LiveRead {
    pub entries: Vec<LiveEntry>,
    /// Pass this as the cursor to read only what came after.
    pub next_cursor: u64,
    /// Entries between the cursor and the oldest kept one: published, then
    /// dropped from the ring before they were read.
    pub dropped: u64,
    /// Entries kept now.
    pub buffered: usize,
}

/// The entries from `cursor` on (0 reads from the oldest kept), those for
/// which `keep` holds, at most `limit`. `limit` keeps the **newest** when more
/// match (a tail), and `next_cursor` is then still the end of the log, so the
/// next read starts after everything seen.
pub fn read_since(cursor: u64, limit: usize, keep: impl Fn(&LiveEntry) -> bool) -> LiveRead {
    let bus = LIVE_LOG_BUS.lock();
    let first = bus.first_seq();
    let start = cursor.max(first);
    let skip = (start - first) as usize;
    let mut entries: Vec<LiveEntry> = bus
        .entries
        .iter()
        .skip(skip)
        .filter(|entry| keep(entry))
        .cloned()
        .collect();
    if entries.len() > limit {
        entries.drain(..entries.len() - limit);
    }
    LiveRead {
        entries,
        next_cursor: bus.next_seq,
        dropped: first.saturating_sub(cursor),
        buffered: bus.entries.len(),
    }
}

/// The cursor that reads only what is published from now on.
pub fn end_cursor() -> u64 {
    LIVE_LOG_BUS.lock().next_seq
}

/// [`read_since`], waiting up to `timeout` for at least one matching entry.
/// Returns as soon as there is one (all of the matches then), or empty when the
/// time is up; `next_cursor` then still moves to the end so a poll loop does not
/// rescan what it already filtered out.
pub fn wait_for_new(
    cursor: u64,
    limit: usize,
    timeout: Duration,
    keep: impl Fn(&LiveEntry) -> bool,
) -> LiveRead {
    let deadline = Instant::now() + timeout;
    loop {
        let read = read_since(cursor, limit, &keep);
        if !read.entries.is_empty() {
            return read;
        }
        let now = Instant::now();
        if now >= deadline {
            return read;
        }
        // Park until a publish (or a short slice, so the deadline holds).
        let mut bus = LIVE_LOG_BUS.lock();
        if bus.next_seq > read.next_cursor {
            continue;
        }
        let slice = (deadline - now).min(Duration::from_millis(250));
        PUBLISHED.wait_for(&mut bus, slice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The bus is process-wide: each test reads from a cursor it took itself and
    // filters on text only it publishes.
    #[test]
    fn a_cursor_reads_only_what_came_after() {
        let cursor = end_cursor();
        publish_live_log("2026-01-01 00:00:00.000 INFO t: cursor-test one");
        publish_live_log("2026-01-01 00:00:00.001 WARN t: cursor-test two");
        let mine = |entry: &LiveEntry| entry.line.contains("cursor-test");
        let read = read_since(cursor, 100, mine);
        assert_eq!(read.entries.len(), 2);
        assert!(read.entries[0].seq < read.entries[1].seq);
        let again = read_since(read.next_cursor, 100, mine);
        assert!(again.entries.is_empty(), "nothing is returned twice");
        publish_live_log("2026-01-01 00:00:00.002 INFO t: cursor-test three");
        let later = read_since(read.next_cursor, 100, mine);
        assert_eq!(later.entries.len(), 1);
        assert!(later.entries[0].line.ends_with("three"));
    }

    #[test]
    fn a_limit_keeps_the_newest_matches() {
        let cursor = end_cursor();
        for n in 0..5 {
            publish_live_log(format!("2026-01-01 00:00:00.000 INFO t: tail-test {n}"));
        }
        let read = read_since(cursor, 2, |entry| entry.line.contains("tail-test"));
        let lines: Vec<&str> = read.entries.iter().map(|e| e.line.as_str()).collect();
        assert!(lines[0].ends_with('3') && lines[1].ends_with('4'), "{lines:?}");
    }

    #[test]
    fn task_attribution_is_kept() {
        let cursor = end_cursor();
        publish_live_log_in_task("2026-01-01 00:00:00.000 INFO t: task-test", Some(42));
        let read = read_since(cursor, 10, |entry| entry.line.contains("task-test"));
        assert_eq!(read.entries[0].task, Some(42));
    }

    #[test]
    fn waiting_returns_when_a_matching_line_arrives_and_times_out_when_none_does() {
        let cursor = end_cursor();
        let publisher = editor_task_queue::spawn_named(
            editor_task_queue::ThreadSpec::new("live log test", editor_task_queue::ThreadRole::Other),
            || {
                std::thread::sleep(Duration::from_millis(150));
                publish_live_log("2026-01-01 00:00:00.000 ERROR t: wait-test arrived");
            },
        );
        let started = Instant::now();
        let read = wait_for_new(cursor, 10, Duration::from_secs(10), |entry| {
            entry.line.contains("wait-test")
        });
        assert_eq!(read.entries.len(), 1);
        assert!(started.elapsed() < Duration::from_secs(5), "returned on arrival");
        publisher.join().unwrap();

        let none = wait_for_new(read.next_cursor, 10, Duration::from_millis(200), |entry| {
            entry.line.contains("wait-test-never")
        });
        assert!(none.entries.is_empty());
        assert!(none.next_cursor >= read.next_cursor);
    }
}
