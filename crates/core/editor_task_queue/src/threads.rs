//! The thread registry: every thread the engine owns or can adopt, with its
//! role, pool, state, and what it is working on.
//!
//! Threads join in three ways, none of which needs the code on the thread to
//! change:
//!
//! * [`spawn_thread`] (and [`TaskQueue::spawn_service`](crate::TaskQueue::spawn_service)
//!   for threads that are persistent tasks) creates the thread, so it is
//!   registered from its first instruction.
//! * [`register_current_thread`] adopts a thread somebody else created. The
//!   pool builders in [`crate::pools`] use it from rayon's and tokio's own
//!   start hooks, so those libraries' workers appear without those libraries
//!   knowing.
//! * The editor's task workers register themselves.
//!
//! State is written by the thread itself into atomics (`mark_running`,
//! `mark_parked`, `set_phase`), so reading the registry never samples the OS
//! and never interrupts a thread. Pools whose library gives no park/unpark
//! hook (rayon) report [`ThreadState::Unknown`] rather than a guess.

use crate::{now_ms, TaskId};
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

/// Exited threads kept so a thread that died is still visible.
const MAX_EXITED: usize = 64;

/// What a thread is for. The Tasks window groups threads by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ThreadRole {
    /// The UI thread.
    Main,
    /// A renderer's thread: a viewport's render loop, GPU setup.
    Render,
    /// A long-lived loop: file watcher, language server, relay client.
    Service,
    /// A worker of the editor task pools.
    EditorWorker,
    /// A tokio runtime worker.
    Async,
    /// A data-parallel pool worker (rayon).
    Compute,
    /// Blocking I/O or a helper that waits on the OS.
    Io,
    Other,
}

impl ThreadRole {
    pub const ALL: [Self; 8] = [
        Self::Main,
        Self::Render,
        Self::Service,
        Self::EditorWorker,
        Self::Async,
        Self::Compute,
        Self::Io,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "UI",
            Self::Render => "Render",
            Self::Service => "Services",
            Self::EditorWorker => "Editor task workers",
            Self::Async => "Async runtime (tokio)",
            Self::Compute => "Compute pool (rayon)",
            Self::Io => "I/O",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    /// The pool gives no signal (rayon), or nothing was reported yet.
    Unknown,
    /// Executing work.
    Running,
    /// Waiting for work (parked on a condvar, a channel, the runtime's queue).
    Parked,
    Exited,
}

impl ThreadState {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Running,
            2 => Self::Parked,
            3 => Self::Exited,
            _ => Self::Unknown,
        }
    }
}

/// How to register a thread.
#[derive(Clone, Debug)]
pub struct ThreadSpec {
    pub name: String,
    pub role: ThreadRole,
    /// The pool it belongs to (`tokio`, `rayon`, `editor-short`), if any.
    pub pool: Option<String>,
    /// The subsystem that owns it, shown in the window.
    pub owner: String,
    pub stack_size: Option<usize>,
}

impl ThreadSpec {
    pub fn new(name: impl Into<String>, role: ThreadRole) -> Self {
        Self {
            name: name.into(),
            role,
            pool: None,
            owner: String::new(),
            stack_size: None,
        }
    }

    pub fn pool(mut self, pool: impl Into<String>) -> Self {
        self.pool = Some(pool.into());
        self
    }

    pub fn owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = owner.into();
        self
    }

    pub fn stack_size(mut self, bytes: usize) -> Self {
        self.stack_size = Some(bytes);
        self
    }
}

struct ThreadShared {
    id: u64,
    name: String,
    role: ThreadRole,
    pool: Option<String>,
    owner: String,
    started_at: SystemTime,
    started_ms: u64,
    state: AtomicU8,
    state_since: AtomicU64,
    /// Total time spent `Running`, excluding the current stretch.
    busy_ms: AtomicU64,
    task: AtomicU64,
    phase: Mutex<Option<String>>,
}

impl ThreadShared {
    fn set_state(&self, next: ThreadState) {
        let now = now_ms();
        let previous = self.state.swap(next as u8, Ordering::AcqRel);
        let since = self.state_since.swap(now, Ordering::AcqRel);
        if ThreadState::from_u8(previous) == ThreadState::Running {
            self.busy_ms
                .fetch_add(now.saturating_sub(since), Ordering::AcqRel);
        }
    }

    fn snapshot(&self, now: u64) -> ThreadSnapshot {
        let state = ThreadState::from_u8(self.state.load(Ordering::Acquire));
        let since = self.state_since.load(Ordering::Acquire);
        let mut busy = self.busy_ms.load(Ordering::Acquire);
        if state == ThreadState::Running {
            busy += now.saturating_sub(since);
        }
        let task = self.task.load(Ordering::Acquire);
        ThreadSnapshot {
            id: self.id,
            name: self.name.clone(),
            role: self.role,
            pool: self.pool.clone(),
            owner: self.owner.clone(),
            state,
            state_for: Duration::from_millis(now.saturating_sub(since)),
            started_at: self.started_at,
            busy: (state != ThreadState::Unknown).then(|| Duration::from_millis(busy)),
            alive_for: Duration::from_millis(now.saturating_sub(self.started_ms)),
            phase: self.phase.lock().clone(),
            task: (task != 0).then_some(TaskId(task)),
        }
    }
}

/// One thread, as the Tasks window shows it.
#[derive(Clone, Debug)]
pub struct ThreadSnapshot {
    pub id: u64,
    pub name: String,
    pub role: ThreadRole,
    pub pool: Option<String>,
    pub owner: String,
    pub state: ThreadState,
    /// How long it has been in `state`.
    pub state_for: Duration,
    pub started_at: SystemTime,
    /// Total time spent running, when the thread reports its state.
    pub busy: Option<Duration>,
    pub alive_for: Duration,
    /// What it last said it is doing.
    pub phase: Option<String>,
    /// The editor task being polled on it right now.
    pub task: Option<TaskId>,
}

#[derive(Default)]
struct Table {
    next_id: u64,
    live: BTreeMap<u64, Arc<ThreadShared>>,
    exited: VecDeque<ThreadSnapshot>,
}

fn table() -> &'static Mutex<Table> {
    static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<ThreadShared>>> = const { RefCell::new(None) };
}

/// Registers the calling thread until dropped. Hold it for the thread's life.
#[must_use = "the thread is unregistered when the guard is dropped"]
pub struct ThreadGuard {
    shared: Arc<ThreadShared>,
}

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        self.shared.set_state(ThreadState::Exited);
        self.shared.task.store(0, Ordering::Release);
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            if current
                .as_ref()
                .is_some_and(|shared| Arc::ptr_eq(shared, &self.shared))
            {
                *current = None;
            }
        });
        let mut table = table().lock();
        table.live.remove(&self.shared.id);
        let snapshot = self.shared.snapshot(now_ms());
        if table.exited.len() >= MAX_EXITED {
            table.exited.pop_front();
        }
        table.exited.push_back(snapshot);
    }
}

/// Adopt the calling thread into the registry. Hold the guard for the thread's
/// life; dropping it moves the thread to the exited list.
pub fn register_current_thread(spec: ThreadSpec) -> ThreadGuard {
    let shared = {
        let mut table = table().lock();
        table.next_id += 1;
        let shared = Arc::new(ThreadShared {
            id: table.next_id,
            name: spec.name,
            role: spec.role,
            pool: spec.pool,
            owner: spec.owner,
            started_at: SystemTime::now(),
            started_ms: now_ms(),
            state: AtomicU8::new(ThreadState::Running as u8),
            state_since: AtomicU64::new(now_ms()),
            busy_ms: AtomicU64::new(0),
            task: AtomicU64::new(0),
            phase: Mutex::new(None),
        });
        table.live.insert(shared.id, Arc::clone(&shared));
        shared
    };
    CURRENT.with(|current| *current.borrow_mut() = Some(Arc::clone(&shared)));
    ThreadGuard { shared }
}

/// A handle to the calling thread's registry entry.
#[derive(Clone)]
pub struct ThreadHandle {
    shared: Arc<ThreadShared>,
}

/// The calling thread's entry, if it is registered.
pub fn current_thread() -> Option<ThreadHandle> {
    CURRENT.with(|current| {
        current.borrow().as_ref().map(|shared| ThreadHandle {
            shared: Arc::clone(shared),
        })
    })
}

impl ThreadHandle {
    pub fn mark_running(&self) {
        self.shared.set_state(ThreadState::Running);
    }

    /// For pools that give no park/unpark signal: state is not known.
    pub fn mark_unknown(&self) {
        self.shared.set_state(ThreadState::Unknown);
    }

    pub fn mark_parked(&self) {
        self.shared.set_state(ThreadState::Parked);
    }

    /// Say what the thread is doing, e.g. "Compiling pipelines".
    pub fn set_phase(&self, phase: impl Into<String>) {
        *self.shared.phase.lock() = Some(phase.into());
    }

    pub fn clear_phase(&self) {
        *self.shared.phase.lock() = None;
    }
}

/// Record which task the calling thread is polling. Returns the previous one.
pub(crate) fn set_current_task(id: u64) -> u64 {
    CURRENT.with(|current| {
        current
            .borrow()
            .as_ref()
            .map_or(0, |shared| shared.task.swap(id, Ordering::AcqRel))
    })
}

/// Spawn a registered thread. The replacement for `std::thread::spawn` in the
/// engine and editor: the thread is named, listed in the Tasks window with its
/// role and owner, and unlisted when it ends.
///
/// For a thread that is a long-lived loop, prefer
/// [`TaskQueue::spawn_service`](crate::TaskQueue::spawn_service): it adds a
/// heartbeat, a log and stall detection.
pub fn spawn_thread<T: Send + 'static>(
    spec: ThreadSpec,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<T>> {
    let mut builder = std::thread::Builder::new().name(spec.name.clone());
    if let Some(bytes) = spec.stack_size {
        builder = builder.stack_size(bytes);
    }
    builder.spawn(move || {
        let _registered = register_current_thread(spec);
        work()
    })
}

/// [`spawn_thread`] for call sites that, like `std::thread::spawn`, treat a
/// failure to start a thread as fatal. Returns the same `JoinHandle`, so it is
/// a drop-in replacement.
pub fn spawn_named<T: Send + 'static>(
    spec: ThreadSpec,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    let name = spec.name.clone();
    spawn_thread(spec, work)
        .unwrap_or_else(|error| panic!("failed to spawn thread \"{name}\": {error}"))
}

/// Every registered thread: live ones first (by registration order), then
/// recently exited ones.
pub fn thread_snapshots() -> Vec<ThreadSnapshot> {
    let table = table().lock();
    let now = now_ms();
    let mut snapshots: Vec<ThreadSnapshot> = table
        .live
        .values()
        .map(|shared| shared.snapshot(now))
        .collect();
    snapshots.extend(table.exited.iter().cloned());
    snapshots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(name: &str) -> Option<ThreadSnapshot> {
        thread_snapshots().into_iter().find(|t| t.name == name)
    }

    #[test]
    fn spawned_thread_is_listed_while_alive_and_kept_as_exited_after() {
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let (ready, wait_ready) = std::sync::mpsc::channel::<()>();
        let handle = spawn_thread(
            ThreadSpec::new("test-thread-lifecycle", ThreadRole::Io)
                .pool("test")
                .owner("tests"),
            move || {
                ready.send(()).unwrap();
                let _ = gate.recv();
                7
            },
        )
        .unwrap();
        wait_ready.recv().unwrap();
        let live = find("test-thread-lifecycle").expect("listed");
        assert_eq!(live.role, ThreadRole::Io);
        assert_eq!(live.pool.as_deref(), Some("test"));
        assert_eq!(live.state, ThreadState::Running);

        release.send(()).unwrap();
        assert_eq!(handle.join().unwrap(), 7);
        let gone = find("test-thread-lifecycle").expect("kept after exit");
        assert_eq!(gone.state, ThreadState::Exited);
    }

    #[test]
    fn state_phase_and_busy_time_are_reported_by_the_thread() {
        let _guard =
            register_current_thread(ThreadSpec::new("test-thread-state", ThreadRole::Service));
        let me = current_thread().expect("registered");
        me.set_phase("compiling");
        me.mark_parked();
        let parked = find("test-thread-state").unwrap();
        assert_eq!(parked.state, ThreadState::Parked);
        assert_eq!(parked.phase.as_deref(), Some("compiling"));

        me.mark_running();
        std::thread::sleep(Duration::from_millis(20));
        let running = find("test-thread-state").unwrap();
        assert_eq!(running.state, ThreadState::Running);
        assert!(running.busy.unwrap() >= Duration::from_millis(15));
        me.clear_phase();
        assert!(find("test-thread-state").unwrap().phase.is_none());
    }

    #[test]
    fn a_task_polled_on_a_registered_thread_is_named_on_it() {
        let _guard = register_current_thread(ThreadSpec::new("test-thread-task", ThreadRole::Main));
        let queue = crate::TaskQueue::new();
        queue.scope(
            crate::TaskDescription::foreground("on a thread", "test"),
            |ctx| {
                let on_thread = find("test-thread-task").unwrap();
                assert_eq!(on_thread.task, Some(ctx.id()));
            },
        );
        assert_eq!(find("test-thread-task").unwrap().task, None);
    }
}
