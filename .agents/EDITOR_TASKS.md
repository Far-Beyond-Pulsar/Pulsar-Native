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

Not yet enforced (be honest about it when you touch these): raw `cx.spawn` and
`std::thread::spawn` in editor and plugin code (dozens of files) are not
blocked by a lint. Convert them to the entry points above when you change them;
do not add new ones. A shrink-only allowlist check like
`architecture/kind-names.txt` is the way to enforce it.

## Adding to this system

* New status or diagnostic: extend `TaskSnapshot` and `diagnose` in `lib.rs`;
  the window reads snapshots only.
* New inline framework path: call `scope`, `set_phase` per stage, log with
  `tracing`, return a `Result` so failures are recorded.
* Tests: `crates/core/editor_task_queue/src/tests.rs`. Use `queue.track` and a
  `futures::future::join` on one thread to prove log attribution under
  interleaving, and `set_stall_thresholds` with millisecond values to test
  stalls.
