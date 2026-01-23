//! Ultra-fast instrumentation-based profiling inspired by Unreal Insights
//! 
//! Unlike sampling profilers that interrupt threads, this uses explicit instrumentation
//! macros that are compiled in and record exact timing with minimal overhead.
//!
//! # Usage
//! 
//! ```rust
//! use profiling::profile_scope;
//! 
//! fn expensive_function() {
//!     profile_scope!("expensive_function");
//!     // Your code here - timing is automatically captured
//! }
//! ```

use std::sync::Arc;
use std::time::Instant;
use parking_lot::RwLock;
use crossbeam_channel::{Sender, Receiver, unbounded};
use serde::{Serialize, Deserialize};

pub mod database;

/// A profiling event captured via instrumentation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEvent {
    /// Event name (function/scope name)
    pub name: String,
    /// Thread ID
    pub thread_id: u64,
    /// Thread name (if set)
    pub thread_name: Option<String>,
    /// Process ID
    pub process_id: u32,
    /// Parent scope name (if nested)
    pub parent_name: Option<String>,
    /// Start time in nanoseconds (absolute)
    pub start_ns: u64,
    /// Duration in nanoseconds
    pub duration_ns: u64,
    /// Stack depth / nesting level
    pub depth: u32,
    /// File location (file:line)
    pub location: Option<String>,
    /// Additional metadata (optional)
    pub metadata: Option<String>,
}

/// Thread-local profiling state
struct ThreadState {
    depth: u32,
    start_time: Option<Instant>,
    scope_stack: Vec<String>, // Track parent scopes
}

impl Default for ThreadState {
    fn default() -> Self {
        Self {
            depth: 0,
            start_time: None,
            scope_stack: Vec::new(),
        }
    }
}

thread_local! {
    static THREAD_STATE: std::cell::RefCell<ThreadState> = std::cell::RefCell::new(ThreadState::default());
    static THREAD_NAME: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}

/// Global profiler state
static PROFILER: once_cell::sync::Lazy<Profiler> = once_cell::sync::Lazy::new(|| Profiler::new());

struct Profiler {
    enabled: Arc<RwLock<bool>>,
    sender: Sender<ProfileEvent>,
    receiver: Receiver<ProfileEvent>,
    events: Arc<RwLock<Vec<ProfileEvent>>>,
    process_id: u32,
}

impl Profiler {
    fn new() -> Self {
        let (sender, receiver) = unbounded();
        Self {
            enabled: Arc::new(RwLock::new(false)),
            sender,
            receiver,
            events: Arc::new(RwLock::new(Vec::new())),
            process_id: std::process::id(),
        }
    }

    fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    fn enable(&self) {
        *self.enabled.write() = true;
    }

    fn disable(&self) {
        *self.enabled.write() = false;
    }

    fn submit_event(&self, event: ProfileEvent) {
        let _ = self.sender.send(event);
    }

    fn collect_events(&self) -> Vec<ProfileEvent> {
        let mut collected = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            collected.push(event);
        }
        
        let mut events = self.events.write();
        events.extend(collected.iter().cloned());
        collected
    }

    fn get_all_events(&self) -> Vec<ProfileEvent> {
        self.events.read().clone()
    }

    fn clear(&self) {
        self.events.write().clear();
        while self.receiver.try_recv().is_ok() {}
    }

    fn get_process_id(&self) -> u32 {
        self.process_id
    }
}

/// Set a name for the current thread (improves readability in traces)
pub fn set_thread_name(name: impl Into<String>) {
    THREAD_NAME.with(|tn| {
        *tn.borrow_mut() = Some(name.into());
    });
}

/// Enable profiling globally
pub fn enable_profiling() {
    PROFILER.enable();
}

/// Disable profiling globally
pub fn disable_profiling() {
    PROFILER.disable();
}

/// Check if profiling is enabled
pub fn is_profiling_enabled() -> bool {
    PROFILER.is_enabled()
}

/// Collect all events captured so far (non-destructive)
pub fn collect_events() -> Vec<ProfileEvent> {
    PROFILER.collect_events()
}

/// Get all captured events
pub fn get_all_events() -> Vec<ProfileEvent> {
    PROFILER.get_all_events()
}

/// Clear all captured events
pub fn clear_events() {
    PROFILER.clear();
}

/// Record a frame time (for FPS tracking in profiler UI)
pub fn record_frame_time(frame_time_ms: f32) {
    if !PROFILER.is_enabled() {
        return;
    }

    // Submit a special frame marker event
    let event = ProfileEvent {
        name: "__FRAME_MARKER__".to_string(),
        thread_id: get_thread_id(),
        thread_name: THREAD_NAME.with(|tn| tn.borrow().clone()),
        process_id: PROFILER.get_process_id(),
        parent_name: None,
        start_ns: get_time_ns(),
        duration_ns: (frame_time_ms * 1_000_000.0) as u64, // Store frame time in duration field
        depth: 0,
        location: None,
        metadata: Some(format!("frame_time_ms:{}", frame_time_ms)),
    };
    
    PROFILER.submit_event(event);
}

/// RAII scope guard for profiling
pub struct ProfileScope {
    name: String,
    start: Instant,
    start_ns: u64,
    depth: u32,
    thread_id: u64,
    thread_name: Option<String>,
    parent_name: Option<String>,
    location: Option<String>,
}

impl ProfileScope {
    /// Begin a new profiling scope
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_location(name, None)
    }

    /// Begin a new profiling scope with file location
    pub fn new_with_location(name: impl Into<String>, location: Option<String>) -> Self {
        if !PROFILER.is_enabled() {
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
        if !PROFILER.is_enabled() {
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
            process_id: PROFILER.get_process_id(),
            parent_name: self.parent_name.clone(),
            start_ns: self.start_ns,
            duration_ns,
            depth: self.depth,
            location: self.location.clone(),
            metadata: None,
        };

        PROFILER.submit_event(event);
    }
}

/// Get current time in nanoseconds
fn get_time_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Get current thread ID
fn get_thread_id() -> u64 {
    // Use a hash of the thread ID instead of unstable API
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let thread_id = std::thread::current().id();
    let mut hasher = DefaultHasher::new();
    thread_id.hash(&mut hasher);
    hasher.finish()
}

/// Macro for profiling a scope with a static name
#[macro_export]
macro_rules! profile_scope {
    ($name:expr) => {
        let _profile_guard = $crate::ProfileScope::new($name);
    };
}

/// Macro for profiling a scope with file/line information
#[macro_export]
macro_rules! profile_scope_loc {
    ($name:expr) => {
        let _profile_guard = $crate::ProfileScope::new_with_location(
            $name,
            Some(format!("{}:{}", file!(), line!()))
        );
    };
}

/// Macro for profiling a function (uses function name automatically)
#[macro_export]
macro_rules! profile_function {
    () => {
        $crate::profile_scope!(module_path!());
    };
}

// Add once_cell dependency
// Lazy removed - not currently used
// use once_cell::sync::Lazy;
