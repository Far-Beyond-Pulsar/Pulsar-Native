//! RAII scope guard for profiling

use once_cell::sync::OnceCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::thread;
use std::time::Instant;

use crate::events::ProfileEvent;
use crate::profiler::Profiler;

static PROFILER: OnceCell<Profiler> = OnceCell::new();

pub fn init_profiler() -> &'static Profiler {
    PROFILER.get_or_init(|| Profiler::new())
}

/// Thread-local profiling state
#[derive(Default)]
pub struct ThreadState {
    depth: u32,
    start_time: Option<Instant>,
    scope_stack: Vec<String>, // Track parent scopes
}

thread_local! {
    pub(super) static THREAD_STATE: std::cell::RefCell<ThreadState> = std::cell::RefCell::new(ThreadState::default());
    pub(super) static THREAD_NAME: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// RAII scope guard for profiling
pub struct ProfileScope {
    pub(super) name: String,
    start: Instant,
    start_ns: u64,
    depth: u32,
    thread_id: u64,
    thread_name: Option<String>,
    parent_name: Option<String>,
    pub(super) location: Option<String>,
}

impl ProfileScope {
    /// Begin a new profiling scope
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_location(name, None)
    }

    /// Begin a new profiling scope with file location
    pub fn new_with_location(name: impl Into<String>, location: Option<String>) -> Self {
        if !init_profiler().is_enabled() {
            return Self {
                name: String::new(),
                start: Instant::now(),
                start_ns: 0,
                depth: 0,
                thread_id: 0,
                thread_name: None,
                parent_name: None,
                location: None,
            };
        }

        let name = name.into();
        let start = Instant::now();
        let start_ns = get_time_ns();
        let thread_id = get_thread_id();
        let thread_name = THREAD_NAME.with(|tn| tn.borrow().clone());

        let (depth, parent_name) = THREAD_STATE.with(|ts| {
            let mut state = ts.borrow_mut();
            if state.start_time.is_none() {
                state.start_time = Some(Instant::now());
            }
            let depth = state.depth;
            state.depth += 1;

            // Get parent scope name
            let parent = state.scope_stack.last().cloned();
            state.scope_stack.push(name.clone());

            (depth, parent)
        });

        Self {
            name,
            start,
            start_ns,
            depth,
            thread_id,
            thread_name,
            parent_name,
            location,
        }
    }
}

impl Drop for ProfileScope {
    fn drop(&mut self) {
        if !init_profiler().is_enabled() {
            return;
        }

        let duration_ns = self.start.elapsed().as_nanos() as u64;

        THREAD_STATE.with(|ts| {
            let mut state = ts.borrow_mut();
            state.depth = state.depth.saturating_sub(1);
            state.scope_stack.pop();
        });

        let event = ProfileEvent {
            name: self.name.clone(),
            thread_id: self.thread_id,
            thread_name: self.thread_name.clone(),
            process_id: init_profiler().get_process_id(),
            parent_name: self.parent_name.clone(),
            start_ns: self.start_ns,
            duration_ns,
            depth: self.depth,
            location: self.location.clone(),
            metadata: None,
        };

        init_profiler().submit_event(event);
    }
}

fn get_time_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn get_thread_id() -> u64 {
    let thread_id = thread::current().id();
    let mut hasher = DefaultHasher::new();
    thread_id.hash(&mut hasher);
    hasher.finish()
}
