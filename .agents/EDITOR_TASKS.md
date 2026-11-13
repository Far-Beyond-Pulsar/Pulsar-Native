# Editor tasks

Everything the editor does that is more than one GPUI callback runs as a **task**
in `editor_task_queue`. A task can be listed, logged, audited, cancelled, and
diagnosed when it stalls. This is the way to find out why a tab did not load or
why a viewport stayed blank: open the Tasks window (status bar), find the task,
read its log.

Game and runtime work never goes here (`pulsar_game` has its own scheduling).

## The rules

1. **Do not spawn editor work by hand.** No `std::thread::spawn`, `cx.spawn`,
   `smol::spawn`, `tokio::spawn` for work whose progress or failure someone may
   need to understand. Use one of the four entry points below.
2. **Log with `tracing`, inside the task.** Do not invent a log sink. Anything
   emitted while the task's future is being polled lands in that task's log.
3. **Name your phases.** `ctx.set_phase("Reading level file")` before each stage.
   A stall report says which phase the task stopped in; without phases it says
   "none set".
4. **Wait with `.await`, never by blocking inside async code.** A task that holds
   its worker inside one poll is reported as blocked.
5. **Framework paths are enforced.** Building an editor tab and Helio's
   first-frame initialisation always run as tasks. Plugins do not opt in and
   cannot opt out; do not add a second way to build an editor or a renderer.

## Entry points

| Call | Use for | Runs |
|---|---|---|
| `global().submit_async(desc, \|ctx\| async move { .. })` | I/O and waiting; the default | pool worker, as a future (need not be `Send`) |
| `global().submit(desc, \|ctx\| { .. })` | blocking work: builds, imports, thumbnails | pool worker (blocks it by design) |
| `editor_task_queue::tracked(desc, future)` | a future an executor already owns, e.g. inside `cx.spawn` | that executor |
| `editor_task_queue::scope(desc, \|ctx\| { .. })` | work that must stay on the calling thread: building a GPUI entity, first-frame GPU init | inline |

`scope` and `tracked` return the work's own value; a `Result` return makes
`Err` the task's failure, a panic is recorded with its message and then keeps
unwinding. `TaskDescription::new(title, category, Short | Long)` for pooled
work, `TaskDescription::foreground(title, category)` for `scope`.

Short work shares the on-demand pool, long work the dedicated pool, so a build
cannot starve a short interaction. Starring a queued task raises its priority.

## Logs belong to tasks through the async runtime

Each task owns a bounded log buffer (5000 lines). Lines get there two ways:

* **Spans.** Every task runs in a root `editor_task` tracing span
  (`task_id`, `title`, `category`). The future is wrapped in `Tracked`, which
  enters that span for the duration of **each poll** and leaves it when the poll
  returns. `TaskLogLayer` (installed in `engine/src/logging.rs`) routes any event
  emitted inside the span to the task's buffer. Two tasks interleaving on one
  thread therefore cannot mix lines: a line goes to whichever task was being
  polled. The same lines also reach `engine.log` and Mission Control, prefixed
  with the span (`editor_task{task_id=7 ..}`).
* **Lifecycle lines.** The queue writes queued/started/phase/stalled/finished
  directly into the buffer, so they exist even with no subscriber installed.
  Direct lines carry a `task_direct` field so the layer does not add them twice.

Work you start from a task does not inherit the span. `tokio::spawn`,
`std::thread::spawn` and executors detach from it. Instrument the child with the
parent's span (`future.instrument(ctx.span())`) or `ctx.span().in_scope(..)`,
or give the child its own task. A line logged from an uninstrumented thread
goes to the engine log only, never to a task.

The span is created at ERROR level on purpose, so a restrictive `RUST_LOG`
still keeps tasks attributable. Events inside it still obey `RUST_LOG`: set
`RUST_LOG=debug` to get `debug!` lines in a task's log.

## Auditing stalls

Each snapshot (`global().snapshots()`) carries the live state: `activity`
(`Polling`, `Pending`, `Finished`), how long in it, polls, idle time since the
last log/progress/phase, `worker`, `origin` (`file:line` of the code that created
the task), and `stall: Option<Stall>`. A watchdog thread (1s) logs a `stalled:`
warning into the task's log when a stall starts and `no longer stalled` when it
clears. Defaults (`StallThresholds`):

| Kind | Meaning | After |
|---|---|---|
| `QueueWait` | queued, every worker in the pool is busy | 10s |
| `NeverPolled` | a `tracked` future nobody polled (built, not awaited or spawned) | 5s |
| `BlockedPoll` | an async task inside one poll: sync work in async code | 2s |
| `BlockedThread` | a `scope` task holding its (UI or render) thread | 1s |
| `Pending` | an async task awaiting, no log/progress/phase change | 15s |
| `NoProgress` | a blocking task, no log/progress/phase change | 30s |

A future that is dropped before it finishes ends `Cancelled` with "dropped
before it completed", which is what a cancelled-by-drop `cx.spawn` looks like.

## The Tasks window

`ui_log_viewer::components::task_panel` (window name `EditorTasksWindow`, opened
from the status bar by name). It lives in the Mission Control crate, a tool,
because it embeds that crate's `LogDrawer`; the kernel shell opens it by name
and names no tool. List tabs: All, Waiting, In Progress, **Stalled**, Failed,
Succeeded. Selecting a row opens the detail pane: origin, thread, model, phase,
what it is doing now, the stall reason, and the task's log in the same table,
level filters and search as Mission Control. `LogDrawer::for_task(Arc<TaskLog>)`
follows the buffer by cursor, so a task that logs fast costs the task nothing.

## Where it is enforced

* `plugin_manager::editors` (`create_editor_for_file_with_project_root`): every
  built-in and plugin editor is created inside `scope("Open editor: <file>",
  "tabs")`. It is the only path from a file to an editor (layout restore,
  waking tabs, the chat panel and manual tools all call it).
* `engine_backend` Helio renderer `prepare_for_render`: the lazy first-frame
  initialisation (scene renderer, render graph, pipelines) runs inside
  `scope("Initialize Helio renderer", "render")`.
* `asset_import`, `ui_build`, `ui_common::thumbnails`, the PIE and Blueprint
  save paths already use `submit`.

Raw `cx.spawn` is not blocked; raw threads, tokio runtimes and rayon pools are
(next section).

## Every thread, not just tasks

Tasks say what work is happening. The thread registry
(`editor_task_queue::threads`) says what every thread is doing, including the
ones that run no task. The Tasks window has four views over them:

| View | Shows |
|---|---|
| **Tasks** | finite work: queued, running, stalled, failed, done. A strip under the tabs summarises **micro tasks** |
| **Threads** | every registered thread grouped by role (UI, Render, Services, Editor task workers, Async (tokio), Compute (rayon), I/O, Other): owner and pool, running or waiting and for how long, busy time, the phase it named, and the task being polled on it. Click a row with a task to open that task's log |
| **Services** | persistent tasks: heartbeat rate, last pass time, phase, stall |
| **Renderers** | the engine's render runtimes (category `categories::RENDER_RUNTIME`): each row is what the runtime says about itself (`TaskSnapshot::details`: owner, scene, size, update mode, state, visible, CPU frame time, graph textures, frames); its log is what its renderer logged while it drew. See `.agents/RENDER_RUNTIMES.md` |

### Persistent tasks (services)

A thread that lives as long as what it serves (a render loop, a file watcher,
a language-server client) is a **persistent task**. It never succeeds; it has a
heartbeat. `spawn_service(ServiceDescription::new(title, category, thread_name,
role).owner(..).heartbeat(every), |ctx| { loop { ctx.heartbeat(); .. } })`
creates the thread, registers it, runs the loop inside the task's span (its
`tracing` output is its log) and ends the task if the loop returns or panics.
Call `ctx.heartbeat()` (or `ctx.tick(work_time)`) once per pass and
`ctx.set_phase(..)` as the loop changes what it does. A loop that waits for
work on purpose calls `ctx.set_idle(true)` (silence is then not a stall) and
`set_idle(false)` when it resumes. `NoHeartbeat` is reported when beats stop
for longer than `heartbeat(every)` (default 5s, `no_heartbeat()` turns it off).
Examples: `engine_backend` `render_runtimes` (the "Helio runtimes" scheduler,
one beat per pass), `pipelines.rs` (the "Render pipeline assets" watcher, idle
between events), `ui_level_editor` `input_thread.rs`.

### Threadless persistent tasks

Something that lives as long as what it serves but is driven by another loop
(a render runtime, which the runtimes' scheduler draws; a game window's
renderer, which its event loop drives) is a `PersistentTask`:
`global().persistent_task(TaskDescription::persistent(title, category),
heartbeat)`. Its owner beats (`ctx.tick(work)` per frame), names phases (its
state), marks itself idle while it does nothing on purpose, enters
`ctx.span()` around its work so its logs are its own, and publishes
`ctx.set_details(vec![(label, value), ..])`, which a view that lists one
category as a table reads. Dropping it ends the task. Categories that a view
lists on their own are consts in `editor_task_queue::categories`.

### Plain threads

`spawn_thread(ThreadSpec::new(name, role).pool(..).owner(..), f)` returns the
same `io::Result<JoinHandle>` as `thread::Builder::spawn`; `spawn_named` panics
like `std::thread::spawn`. Use these for helper threads that are not a loop
worth a heartbeat. A thread someone else creates joins with
`register_current_thread(spec)` (hold the guard); `current_thread()` gives a
handle for `set_phase` / `mark_running` / `mark_parked`. State lives in atomics
the thread writes itself, so reading the registry never samples the OS.

### Micro tasks

`micro("name", || work)` keeps only per-name counters (count, total, max). Use
it for work measured in microseconds where a record, span and log would cost
more than the work. A single run over 50ms is also kept as its own finished
task (`Slow micro task: name`), so an outlier is not lost in the mean.

### The engine's rayon pool and tokio runtime

`editor_task_queue::pools` (called from `engine/src/runtime.rs` at startup):

* `install_rayon_global(None)`: rayon's global pool is built with a
  `spawn_handler`, so every worker is registered (role Compute, pool `rayon`).
  `par_iter`, `rayon::join` and `rayon::spawn` anywhere then run on it. rayon has
  no park hook, so its workers show state "Not reported".
* `build_tokio_runtime(workers, on_start)`: workers register on start and report
  parked/running through tokio's park/unpark hooks. `tokio_handle()` is the
  engine runtime for code that needs async work and has no runtime;
  `tokio_metrics()` gives workers, live tasks and queue depth. Per-task hooks
  need `--cfg tokio_unstable`, which is not enabled: it must be added to every
  `[target.*] rustflags` in `.cargo/config.toml` (they override `[build]`) and
  rebuilds the world.

What this cannot capture: a crate that builds its **own** pool or runtime
(`ThreadPoolBuilder::new().build()`, `Runtime::new()`, `reqwest::blocking`).

### Helio-layer threads and jobs (`helio_tasks`)

Helio and the plugins' feature and world crates (the voxel world, its
residency and sun planners, its query workers, Helio's encoder-finish pool,
its shader watcher, pipeline compiles) cannot depend on this crate. They
start threads through Helio's `helio_tasks` seam instead:
`helio_tasks::spawn_service(ServiceSpec::new(title, thread).owner(..), |ctx| ..)`
for a loop (wait for work inside `ctx.wait(|| inbox.recv())`, so a host tells
idle from stalled) and `helio_tasks::submit(JobSpec::new(title), |ctx| ..)` for
one piece of blocking work. `engine/src/runtime.rs` installs
`editor_task_queue::helio::install()` at startup, beside the rayon pool: from
then on each such service is a persistent task here (its thread registered
with its role and owner, a heartbeat, idle while waiting) and each job a
blocking task on the dedicated pool. A process that installs nothing (Helio's
own examples and tests, games) runs them on named std threads.

### The raw-thread ratchet

`crates/core/editor_task_queue/tests/raw_threads.rs` fails when a file in
`crates/core`, `crates/editor`, `crates/subsystems`, `crates/profiling` or
`plugins/vendor` calls `thread::spawn`, `thread::Builder::new`,
`Runtime::new`, `Builder::new_multi_thread`/`new_current_thread` or
`ThreadPoolBuilder::new` (outside `#[cfg(test)]` items; code after a test
module is checked, and a call is matched as written, so `ScriptRuntime::new`
is not `Runtime::new`) and is not listed, with its
reason, in `architecture/raw-threads-allowed.txt`. The list only shrinks: do not
add a line, use the task system; remove a line when you migrate a file. What is
left is mostly plugin repositories (separate submodules), private runtimes in
`ui_multiplayer`/`ui_fab_search`/`friends_engine`, and Mission Control's own
allocator-tracking threads.

## Adding to this system

* New status or diagnostic: extend `TaskSnapshot` and `diagnose` in `lib.rs`;
  the window reads snapshots only.
* New inline framework path: call `scope`, `set_phase` per stage, log with
  `tracing`, return a `Result` so failures are recorded.
* Tests: `crates/core/editor_task_queue/src/tests.rs`. Use `queue.track` and a
  `futures::future::join` on one thread to prove log attribution under
  interleaving, and `set_stall_thresholds` with millisecond values to test
  stalls.
