use once_cell::sync::Lazy;
use parking_lot::Mutex;
use smol::channel::{self, Receiver, Sender, TrySendError};
use std::collections::VecDeque;

const LIVE_LOG_BUFFER: usize = 4000;
const LIVE_LOG_CHANNEL_CAPACITY: usize = 2048;

struct LiveLogBus {
    subscribers: Vec<Sender<String>>,
    recent: VecDeque<String>,
}

impl LiveLogBus {
    fn new() -> Self {
        Self {
            subscribers: Vec::new(),
            recent: VecDeque::with_capacity(LIVE_LOG_BUFFER),
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

    bus.subscribers.retain(|tx| match tx.try_send(line.clone()) {
        Ok(_) => true,
        Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Closed(_)) => false,
    });
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
