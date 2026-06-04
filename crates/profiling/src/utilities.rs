//! Utility functions for profiling

use crate::events::ProfileEvent;
use crate::profiler::Profiler;
use crate::scope::{init_profiler, THREAD_NAME};

pub fn set_thread_name(name: impl Into<String>) {
    THREAD_NAME.with(|tn| {
        *tn.borrow_mut() = Some(name.into());
    });
}

pub fn enable_profiling(profiler: &Profiler) {
    profiler.enable();
}

pub fn disable_profiling(profiler: &Profiler) {
    profiler.disable();
}

pub fn is_profiling_enabled(profiler: &Profiler) -> bool {
    profiler.is_enabled()
}

pub fn collect_events(profiler: &Profiler) -> Vec<ProfileEvent> {
    profiler.collect_events()
}

pub fn get_all_events(profiler: &Profiler) -> Vec<ProfileEvent> {
    profiler.get_all_events()
}

pub fn clear_events(profiler: &Profiler) {
    profiler.clear();
}

pub fn record_frame_time(profiler: &Profiler, frame_time_ms: f32) {
    if !profiler.is_enabled() {
        return;
    }

    let event = ProfileEvent {
        name: "__FRAME_MARKER__".to_string(),
        thread_id: get_thread_id(),
        thread_name: THREAD_NAME.with(|tn| tn.borrow().clone()),
        process_id: profiler.get_process_id(),
        parent_name: None,
        start_ns: get_time_ns(),
        duration_ns: (frame_time_ms * 1_000_000.0) as u64,
        depth: 0,
        location: None,
        metadata: Some(format!("frame_time_ms:{}", frame_time_ms)),
    };

    profiler.submit_event(event);
}

fn get_thread_id() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let thread_id = std::thread::current().id();
    let mut hasher = DefaultHasher::new();
    thread_id.hash(&mut hasher);
    hasher.finish()
}

fn get_time_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
