//! Global profiler state management

use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::RwLock;
use std::sync::Arc;

use crate::events::ProfileEvent;

/// Global profiler state
pub struct Profiler {
    enabled: Arc<RwLock<bool>>,
    sender: Sender<ProfileEvent>,
    receiver: Receiver<ProfileEvent>,
    events: Arc<RwLock<Vec<ProfileEvent>>>,
    process_id: u32,
}

impl Profiler {
    pub fn new() -> Self {
        let (sender, receiver) = unbounded();
        Self {
            enabled: Arc::new(RwLock::new(false)),
            sender,
            receiver,
            events: Arc::new(RwLock::new(Vec::new())),
            process_id: std::process::id(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    pub fn enable(&self) {
        *self.enabled.write() = true;
    }

    pub fn disable(&self) {
        *self.enabled.write() = false;
    }

    pub fn submit_event(&self, event: ProfileEvent) {
        let _ = self.sender.send(event);
    }

    pub fn collect_events(&self) -> Vec<ProfileEvent> {
        let mut collected = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            collected.push(event);
        }

        let mut events = self.events.write();
        events.extend(collected.iter().cloned());
        collected
    }

    pub fn get_all_events(&self) -> Vec<ProfileEvent> {
        self.events.read().clone()
    }

    pub fn clear(&self) {
        self.events.write().clear();
        while self.receiver.try_recv().is_ok() {}
    }

    pub fn get_process_id(&self) -> u32 {
        self.process_id
    }
}
