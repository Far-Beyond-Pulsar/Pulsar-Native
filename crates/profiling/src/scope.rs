//! RAII scope guard for profiling

use once_cell::sync::OnceCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use crate::events::ProfileEvent;
use crate::profiler::Profiler;

static PROFILER: OnceCell<Profiler> = OnceCell::new();
static NEXT_SCOPE_ID: AtomicU64 = AtomicU64::new(1);

pub fn allocate_scope_id() -> u64 {
    NEXT_SCOPE_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn init_profiler() -> &'static Profiler {
    PROFILER.get_or_init(|| Profiler::new())
}

/// Thread-local profiling state
#[derive(Default)]
pub struct ThreadState {
    scope_stack: Vec<ScopeFrame>,
    thread_id: Option<u64>,
}

struct ScopeFrame {
    name: ScopeName,
    id: u64,
}

enum ScopeName {
    Static(&'static str),
    Owned(String),
}

impl ScopeName {
    #[inline]
    fn as_str(&self) -> &str {
        match self {
            Self::Static(name) => name,
            Self::Owned(name) => name,
        }
    }

    #[inline]
    fn into_string(self) -> String {
        match self {
            Self::Static(name) => name.to_owned(),
            Self::Owned(name) => name,
        }
    }
}

thread_local! {
    pub(super) static THREAD_STATE: std::cell::RefCell<ThreadState> = std::cell::RefCell::new(ThreadState::default());
    pub(super) static THREAD_NAME: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    pub(super) static TRACK_NAME: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// RAII scope guard for profiling
pub struct ProfileScope {
    /// Only present for an active scope. Keeping this optional lets the
    /// disabled path avoid touching the platform clock at all; profiling
    /// instrumentation is present throughout the engine and must be almost
    /// free when the viewer is not recording.
    start: Option<Instant>,
    start_ns: u64,
    depth: u32,
    thread_id: u64,
    scope_id: u64,
    parent_scope_id: Option<u64>,
    track_name: Option<String>,
    active: bool,
    pub(super) location: Option<String>,
}

impl ProfileScope {
    /// Begin a new profiling scope
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_name(ScopeName::Owned(name.into()), None)
    }

    /// Begin a scope whose logical parent was captured on another thread or
    /// queue. The parent relationship is identity-based, not name-based.
    pub fn new_with_context(name: impl Into<String>, context: ScopeContext) -> Self {
        Self::new_name_with_parent(
            ScopeName::Owned(name.into()),
            None,
            context.parent_scope_id,
            Some(context.depth + 1),
            context.track_name,
        )
    }

    /// Begin a scope whose label is a compile-time string.
    ///
    /// The macro uses this path for string literals so the hot path does not
    /// allocate a label until the event is actually emitted.
    pub fn new_static(name: &'static str) -> Self {
        Self::new_name(ScopeName::Static(name), None)
    }

    /// Begin a new profiling scope with file location
    pub fn new_with_location(name: impl Into<String>, location: Option<String>) -> Self {
        Self::new_name(ScopeName::Owned(name.into()), location)
    }

    /// Begin a static-label scope with file location.
    pub fn new_static_with_location(
        name: &'static str,
        location: Option<String>,
    ) -> Self {
        Self::new_name(ScopeName::Static(name), location)
    }

    fn new_name(name: ScopeName, location: Option<String>) -> Self {
        Self::new_name_with_parent(name, location, None, None, None)
    }

    fn new_name_with_parent(
        name: ScopeName,
        location: Option<String>,
        explicit_parent: Option<u64>,
        explicit_depth: Option<u32>,
        explicit_track: Option<String>,
    ) -> Self {
        if !init_profiler().is_enabled() {
            return Self {
                start: None,
                start_ns: 0,
                depth: 0,
                thread_id: 0,
                scope_id: 0,
                parent_scope_id: None,
                track_name: None,
                active: false,
                location: None,
            };
        }

        let start = Instant::now();
        let start_ns = get_time_ns();
        let (depth, thread_id, parent_scope_id) = THREAD_STATE.with(|ts| {
            let mut state = ts.borrow_mut();
            let depth = explicit_depth.unwrap_or(state.scope_stack.len() as u32);
            let thread_id = *state.thread_id.get_or_insert_with(get_thread_id);
            let parent_scope_id = explicit_parent.or_else(|| state.scope_stack.last().map(|frame| frame.id));
            let scope_id = allocate_scope_id();
            state.scope_stack.push(ScopeFrame { name, id: scope_id });
            (depth, thread_id, parent_scope_id)
        });
        let scope_id = THREAD_STATE.with(|ts| ts.borrow().scope_stack.last().map(|frame| frame.id).unwrap_or(0));

        Self {
            start: Some(start),
            start_ns,
            depth,
            thread_id,
            scope_id,
            parent_scope_id,
            track_name: explicit_track.or_else(|| TRACK_NAME.with(|track| track.borrow().clone())),
            active: true,
            location,
        }
    }
}

impl Drop for ProfileScope {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let Some(start) = self.start else {
            return;
        };
        let duration_ns = start.elapsed().as_nanos() as u64;

        let (name, parent_name) = THREAD_STATE.with(|ts| {
            let mut state = ts.borrow_mut();
            let frame = state
                .scope_stack
                .pop()
                .expect("profiling scope stack must match scope guards");
            let parent_name = state
                .scope_stack
                .last()
                .map(|frame| frame.name.as_str().to_owned());
            (frame.name.into_string(), parent_name)
        });

        if !init_profiler().is_enabled() {
            return;
        }

        let event = ProfileEvent {
            scope_id: self.scope_id,
            parent_scope_id: self.parent_scope_id,
            name,
            thread_id: self.thread_id,
            thread_name: THREAD_NAME.with(|tn| tn.borrow().clone()),
            process_id: init_profiler().get_process_id(),
            parent_name,
            start_ns: self.start_ns,
            duration_ns,
            depth: self.depth,
            location: self.location.take(),
            metadata: None,
            track_name: self.track_name.clone(),
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

pub(super) fn current_thread_id() -> u64 {
    THREAD_STATE.with(|ts| {
        let mut state = ts.borrow_mut();
        *state.thread_id.get_or_insert_with(get_thread_id)
    })
}

/// Capture the currently active logical scope for work handed to another
/// thread or queue. The context contains identity, not a thread-local stack.
#[derive(Clone, Debug, Default)]
pub struct ScopeContext {
    pub parent_scope_id: Option<u64>,
    pub depth: u32,
    pub track_name: Option<String>,
}

pub fn current_scope_context() -> ScopeContext {
    ScopeContext {
        parent_scope_id: THREAD_STATE.with(|ts| ts.borrow().scope_stack.last().map(|frame| frame.id)),
        depth: THREAD_STATE.with(|ts| ts.borrow().scope_stack.len() as u32),
        track_name: TRACK_NAME.with(|track| track.borrow().clone()),
    }
}
