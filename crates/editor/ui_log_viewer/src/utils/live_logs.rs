use once_cell::sync::Lazy;
use parking_lot::Mutex;
use smol::channel::{self, Receiver, Sender, TrySendError};
use std::collections::VecDeque;

const LIVE_LOG_BUFFER: usize = 4000;
const LIVE_LOG_CHANNEL_CAPACITY: usize = 2048;

struct LiveLogBus {
    subscribers: Vec<Sender<String>>,
    recent: VecDeque<String>,
    /// Lines ever published; the buffer holds only the most recent ones.
    total: u64,
}

impl LiveLogBus {
    fn new() -> Self {
        Self {
            subscribers: Vec::new(),
            recent: VecDeque::with_capacity(LIVE_LOG_BUFFER),
            total: 0,
        }
    }
}

static LIVE_LOG_BUS: Lazy<Mutex<LiveLogBus>> = Lazy::new(|| Mutex::new(LiveLogBus::new()));

pub fn publish_live_log(line: impl Into<String>) {
    let line = line.into();
    let mut bus = LIVE_LOG_BUS.lock();

    if bus.recent.len() >= LIVE_LOG_BUFFER {
        bus.recent.pop_front();
    }
    bus.recent.push_back(line.clone());
    bus.total += 1;

    bus.subscribers
        .retain(|tx| match tx.try_send(line.clone()) {
            Ok(_) => true,
            Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Closed(_)) => false,
        });
}

/// A position in the live log: the number of lines published so far. Pair
/// with [`live_logs_since`].
pub fn live_log_mark() -> u64 {
    LIVE_LOG_BUS.lock().total
}

/// Lines published after `mark`, oldest first. Lines that have already left
/// the bounded buffer are gone; the rest are returned.
pub fn live_logs_since(mark: u64) -> Vec<String> {
    let bus = LIVE_LOG_BUS.lock();
    let first_kept = bus.total - bus.recent.len() as u64;
    let skip = mark.saturating_sub(first_kept) as usize;
    bus.recent.iter().skip(skip).cloned().collect()
}

pub fn subscribe_live_logs() -> Receiver<String> {
    let (tx, rx) = channel::bounded(LIVE_LOG_CHANNEL_CAPACITY);
    let mut bus = LIVE_LOG_BUS.lock();

    for line in bus.recent.iter() {
        if tx.try_send(line.clone()).is_err() {
            break;
        }
    }

    bus.subscribers.push(tx);
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_after_a_mark_are_returned_in_order() {
        let mark = live_log_mark();
        publish_live_log("live-mark-test one");
        publish_live_log("live-mark-test two");
        let since = live_logs_since(mark);
        let ours: Vec<&str> = since
            .iter()
            .map(String::as_str)
            .filter(|l| l.starts_with("live-mark-test"))
            .collect();
        assert_eq!(ours, ["live-mark-test one", "live-mark-test two"]);
        assert!(live_logs_since(live_log_mark() + 1_000).is_empty());
    }
}
