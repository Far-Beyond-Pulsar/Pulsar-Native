use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

fn desc(duration: TaskDuration) -> TaskDescription {
    TaskDescription::new("job", "test", duration)
}

fn snapshot(queue: &TaskQueue, id: TaskId) -> TaskSnapshot {
    queue
        .snapshots()
        .into_iter()
        .find(|s| s.id == id)
        .expect("task record exists")
}

fn status(queue: &TaskQueue, id: TaskId) -> TaskStatus {
    snapshot(queue, id).status
}

fn wait_for(queue: &TaskQueue, id: TaskId, expected: TaskStatus) {
    let start = Instant::now();
    while status(queue, id) != expected {
        assert!(
            start.elapsed() < TIMEOUT,
            "timed out waiting for {expected:?}, last status {:?}",
            status(queue, id)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Queue with one worker per pool, whose short worker is held busy until
/// the returned sender is signalled (or dropped).
fn queue_with_blocked_short_worker() -> (TaskQueue, TaskId, mpsc::Sender<()>) {
    let queue = TaskQueue::new();
    queue.set_worker_counts(1, 1);
    let (release, gate) = mpsc::channel::<()>();
    let blocker = queue.submit(desc(TaskDuration::Short), move |_| {
        let _ = gate.recv();
        Ok(())
    });
    wait_for(&queue, blocker, TaskStatus::Running);
    (queue, blocker, release)
}

#[test]
fn successful_task_reports_succeeded_with_full_progress() {
    let queue = TaskQueue::new();
    let id = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    wait_for(&queue, id, TaskStatus::Succeeded);
    let snap = snapshot(&queue, id);
    assert_eq!(snap.progress, Some(1.0));
    assert_eq!(snap.error, None);
    assert_eq!(snap.title, "job");
    assert_eq!(snap.category, "test");
}

#[test]
fn task_ids_are_unique_and_increasing() {
    let queue = TaskQueue::new();
    let a = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    let b = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    assert!(b.get() > a.get());
}

#[test]
fn failing_task_records_error_message() {
    let queue = TaskQueue::new();
    let id = queue.submit(desc(TaskDuration::Short), |_| Err("boom".to_string()));
    wait_for(&queue, id, TaskStatus::Failed);
    assert_eq!(snapshot(&queue, id).error.as_deref(), Some("boom"));
}

#[test]
fn panicking_task_is_failed_and_worker_survives() {
    let queue = TaskQueue::new();
    queue.set_worker_counts(1, 1);
    let bad = queue.submit(desc(TaskDuration::Short), |_| panic!("expected test panic"));
    wait_for(&queue, bad, TaskStatus::Failed);
    assert_eq!(
        snapshot(&queue, bad).error.as_deref(),
        Some("Task panicked")
    );
    assert!(log_text(&queue, bad).contains("panicked: expected test panic"));

    // The only short worker must still be alive to run the next job.
    let ok = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    wait_for(&queue, ok, TaskStatus::Succeeded);
}

#[test]
fn cancelling_queued_task_prevents_it_from_running() {
    let (queue, _blocker, release) = queue_with_blocked_short_worker();
    let ran = Arc::new(AtomicBool::new(false));
    let ran_in_task = Arc::clone(&ran);
    let id = queue.submit(desc(TaskDuration::Short), move |_| {
        ran_in_task.store(true, Ordering::SeqCst);
        Ok(())
    });
    assert_eq!(status(&queue, id), TaskStatus::Queued);

    assert!(queue.cancel(id));
    assert_eq!(status(&queue, id), TaskStatus::Cancelled);
    // Already terminal: a second cancel is rejected.
    assert!(!queue.cancel(id));

    // Let the worker drain, then prove the cancelled job never ran.
    release.send(()).unwrap();
    let marker = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    wait_for(&queue, marker, TaskStatus::Succeeded);
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(status(&queue, id), TaskStatus::Cancelled);
}

#[test]
fn cancel_of_unknown_or_finished_task_returns_false() {
    let queue = TaskQueue::new();
    assert!(!queue.cancel(TaskId(9_999_999)));
    let id = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    wait_for(&queue, id, TaskStatus::Succeeded);
    assert!(!queue.cancel(id));
    assert_eq!(status(&queue, id), TaskStatus::Succeeded);
}

#[test]
fn cooperative_cancel_of_running_task_ends_cancelled() {
    let queue = TaskQueue::new();
    let id = queue.submit(desc(TaskDuration::Short), |ctx| {
        let start = Instant::now();
        while !ctx.is_cancelled() {
            if start.elapsed() > TIMEOUT {
                return Err("never cancelled".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // Even a successful return must not override cancellation.
        Ok(())
    });
    wait_for(&queue, id, TaskStatus::Running);
    assert!(queue.cancel(id));
    wait_for(&queue, id, TaskStatus::Cancelled);
}

#[test]
fn mark_cancelled_from_inside_work_yields_cancelled() {
    let queue = TaskQueue::new();
    let id = queue.submit(desc(TaskDuration::Short), |ctx| {
        ctx.mark_cancelled();
        assert!(ctx.is_cancelled());
        Ok(())
    });
    wait_for(&queue, id, TaskStatus::Cancelled);
}

#[test]
fn context_exposes_its_task_id() {
    let queue = TaskQueue::new();
    let (tx, rx) = mpsc::channel();
    let id = queue.submit(desc(TaskDuration::Short), move |ctx| {
        tx.send(ctx.id()).unwrap();
        Ok(())
    });
    assert_eq!(rx.recv_timeout(TIMEOUT).unwrap(), id);
}

#[test]
fn report_progress_clamps_and_empty_detail_clears() {
    let queue = TaskQueue::new();
    let (reported, wait_reported) = mpsc::channel::<()>();
    let (release, gate) = mpsc::channel::<()>();
    let id = queue.submit(desc(TaskDuration::Short), move |ctx| {
        ctx.report_progress(2.5, "stage one");
        reported.send(()).unwrap();
        gate.recv().unwrap();
        ctx.report_progress(-1.0, "");
        reported.send(()).unwrap();
        gate.recv().unwrap();
        Ok(())
    });

    wait_reported.recv_timeout(TIMEOUT).unwrap();
    let snap = snapshot(&queue, id);
    assert_eq!(snap.progress, Some(1.0));
    assert_eq!(snap.detail.as_deref(), Some("stage one"));

    release.send(()).unwrap();
    wait_reported.recv_timeout(TIMEOUT).unwrap();
    let snap = snapshot(&queue, id);
    assert_eq!(snap.progress, Some(0.0));
    assert_eq!(snap.detail, None);

    release.send(()).unwrap();
    wait_for(&queue, id, TaskStatus::Succeeded);
}

#[test]
fn revision_increases_on_state_changes() {
    let queue = TaskQueue::new();
    let before = queue.revision();
    let id = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    wait_for(&queue, id, TaskStatus::Succeeded);
    assert!(queue.revision() > before);

    let before = queue.revision();
    assert!(queue.set_starred(id, true));
    assert!(queue.revision() > before);

    // Setting the same value is not a change.
    let before = queue.revision();
    assert!(queue.set_starred(id, true));
    assert_eq!(queue.revision(), before);
}

#[test]
fn set_starred_unknown_id_returns_false() {
    let queue = TaskQueue::new();
    assert!(!queue.set_starred(TaskId(9_876_543), true));
}

#[test]
fn starred_queued_task_runs_before_older_unstarred_ones() {
    let (queue, _blocker, release) = queue_with_blocked_short_worker();
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut ids = Vec::new();
    for label in ["a", "b", "c"] {
        let order = Arc::clone(&order);
        ids.push(queue.submit(desc(TaskDuration::Short), move |_| {
            order.lock().push(label);
            Ok(())
        }));
        // Keep submission timestamps strictly ordered.
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(queue.set_starred(ids[2], true));

    release.send(()).unwrap();
    for id in &ids {
        wait_for(&queue, *id, TaskStatus::Succeeded);
    }
    assert_eq!(*order.lock(), vec!["c", "a", "b"]);
}

#[test]
fn long_jobs_do_not_wait_behind_a_blocked_short_pool() {
    let (queue, _blocker, release) = queue_with_blocked_short_worker();
    let long = queue.submit(desc(TaskDuration::Long), |_| Ok(()));
    wait_for(&queue, long, TaskStatus::Succeeded);
    release.send(()).unwrap();
}

#[test]
fn worker_counts_default_and_are_clamped() {
    let queue = TaskQueue::new();
    assert_eq!(
        queue.worker_counts(),
        WorkerCounts {
            on_demand: DEFAULT_ON_DEMAND_WORKERS,
            dedicated: DEFAULT_DEDICATED_WORKERS
        }
    );
    assert_eq!(
        queue.set_worker_counts(0, 1_000),
        WorkerCounts {
            on_demand: 1,
            dedicated: MAX_WORKERS_PER_POOL
        }
    );
    assert_eq!(queue.worker_counts().dedicated, MAX_WORKERS_PER_POOL);
}

#[test]
fn snapshots_are_newest_first() {
    let (queue, blocker, release) = queue_with_blocked_short_worker();
    std::thread::sleep(Duration::from_millis(3));
    let second = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    std::thread::sleep(Duration::from_millis(3));
    let third = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    let ids: Vec<_> = queue.snapshots().iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![third, second, blocker]);
    release.send(()).unwrap();
}

fn quick_thresholds() -> StallThresholds {
    StallThresholds {
        queue_wait: Duration::from_millis(40),
        never_polled: Duration::from_millis(40),
        blocked_poll: Duration::from_millis(40),
        blocked_thread: Duration::from_millis(40),
        pending_idle: Duration::from_millis(40),
        blocking_idle: Duration::from_millis(40),
    }
}

fn log_text(queue: &TaskQueue, id: TaskId) -> String {
    queue
        .log(id)
        .expect("task log")
        .read_from(0)
        .lines
        .join("\n")
}

/// Yields to the executor once, so two futures on one thread interleave.
struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.0 {
            return std::task::Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}

fn with_task_layer<R>(f: impl FnOnce() -> R) -> R {
    use tracing_subscriber::prelude::*;
    let subscriber = tracing_subscriber::registry().with(TaskLogLayer);
    tracing::subscriber::with_default(subscriber, f)
}

#[test]
fn async_task_runs_and_reports_polls() {
    let queue = TaskQueue::new();
    let id = queue.submit_async(desc(TaskDuration::Short), |ctx| async move {
        ctx.set_phase("waiting");
        YieldNow(false).await;
        YieldNow(false).await;
        Ok(())
    });
    wait_for(&queue, id, TaskStatus::Succeeded);
    let snap = snapshot(&queue, id);
    assert_eq!(snap.model, TaskModel::Async);
    assert!(snap.polls >= 3, "polls: {}", snap.polls);
    assert_eq!(snap.phase.as_deref(), Some("waiting"));
    assert!(snap.origin.contains("tests.rs"), "origin: {}", snap.origin);
    assert!(snap
        .worker
        .as_deref()
        .unwrap_or("")
        .starts_with("editor-task-short"));
    assert!(snap.started_at.is_some() && snap.finished_at.is_some());
}

#[test]
fn lifecycle_lines_are_logged_without_a_subscriber() {
    let queue = TaskQueue::new();
    let id = queue.submit(desc(TaskDuration::Short), |ctx| {
        ctx.info("hello from the task");
        Err("boom".into())
    });
    wait_for(&queue, id, TaskStatus::Failed);
    let text = log_text(&queue, id);
    assert!(text.contains("queued: job"), "{text}");
    assert!(text.contains("started on editor-task-short"), "{text}");
    assert!(text.contains("hello from the task"), "{text}");
    assert!(text.contains("failed: boom"), "{text}");
    assert!(snapshot(&queue, id).log_lines >= 4);
}

#[test]
fn tracing_events_are_attributed_to_the_task_being_polled() {
    with_task_layer(|| {
        let queue = TaskQueue::new();
        let a = queue.track(desc(TaskDuration::Short), async {
            tracing::info!("a-one");
            YieldNow(false).await;
            tracing::warn!("a-two");
        });
        let b = queue.track(desc(TaskDuration::Short), async {
            tracing::info!("b-one");
            YieldNow(false).await;
            tracing::error!(code = 7, "b-two");
        });
        let (ida, idb) = (a.id(), b.id());
        // One thread interleaves the two futures poll by poll.
        futures::executor::block_on(futures::future::join(a, b));

        let a_log = log_text(&queue, ida);
        let b_log = log_text(&queue, idb);
        assert!(
            a_log.contains("a-one") && a_log.contains("a-two"),
            "{a_log}"
        );
        assert!(
            b_log.contains("b-one") && b_log.contains("b-two"),
            "{b_log}"
        );
        assert!(
            !a_log.contains("b-one") && !a_log.contains("b-two"),
            "{a_log}"
        );
        assert!(
            !b_log.contains("a-one") && !b_log.contains("a-two"),
            "{b_log}"
        );
        assert!(b_log.contains("code=7"), "{b_log}");
        assert!(a_log.contains(" WARN "), "{a_log}");
    });
}

#[test]
fn events_outside_any_task_are_not_captured() {
    with_task_layer(|| {
        let queue = TaskQueue::new();
        queue.scope(TaskDescription::foreground("t", "test"), |_| ());
        let id = queue.snapshots()[0].id;
        tracing::info!("stray-marker");
        let text = log_text(&queue, id);
        assert!(!text.contains("stray-marker"), "{text}");
    });
}

#[test]
fn direct_lines_are_not_duplicated_by_the_layer() {
    with_task_layer(|| {
        let queue = TaskQueue::new();
        queue.scope(TaskDescription::foreground("t", "test"), |ctx| {
            ctx.info("once only");
        });
        let id = queue.snapshots()[0].id;
        let text = log_text(&queue, id);
        assert_eq!(text.matches("once only").count(), 1, "{text}");
    });
}

#[test]
fn scope_runs_inline_and_records_the_result() {
    let queue = TaskQueue::new();
    let thread = std::thread::current().id();
    let value = queue.scope(TaskDescription::foreground("inline", "test"), |ctx| {
        assert_eq!(std::thread::current().id(), thread);
        assert_eq!(current().map(|c| c.id()), Some(ctx.id()));
        Ok::<_, String>(7)
    });
    assert_eq!(value, Ok(7));
    assert!(current().is_none());
    let snap = &queue.snapshots()[0];
    assert_eq!(snap.status, TaskStatus::Succeeded);
    assert_eq!(snap.duration, TaskDuration::Foreground);
    assert_eq!(snap.model, TaskModel::Foreground);

    let failed = queue.scope(TaskDescription::foreground("bad", "test"), |_| {
        Err::<(), _>("nope")
    });
    assert!(failed.is_err());
    let snap = &queue.snapshots()[0];
    assert_eq!(snap.status, TaskStatus::Failed);
    assert_eq!(snap.error.as_deref(), Some("nope"));
}

#[test]
fn scope_panic_is_recorded_then_resumed() {
    let queue = TaskQueue::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        queue.scope(TaskDescription::foreground("panics", "test"), |_| -> () {
            panic!("kaboom")
        })
    }));
    assert!(result.is_err());
    let snap = queue.snapshots()[0].clone();
    assert_eq!(snap.status, TaskStatus::Failed);
    assert!(log_text(&queue, snap.id).contains("panicked: kaboom"));
    assert!(current().is_none(), "the context must be popped on unwind");
}

#[test]
fn dropped_future_is_cancelled_and_says_so() {
    let queue = TaskQueue::new();
    let never_polled = queue.track(desc(TaskDuration::Short), async {});
    let id = never_polled.id();
    drop(never_polled);
    assert_eq!(status(&queue, id), TaskStatus::Cancelled);
    assert!(log_text(&queue, id).contains("without ever being polled"));
}

#[test]
fn a_blocked_poll_is_reported_as_a_stall_and_logged() {
    let queue = TaskQueue::new();
    queue.set_stall_thresholds(quick_thresholds());
    let (release, gate) = mpsc::channel::<()>();
    let id = queue.submit_async(desc(TaskDuration::Short), move |_| async move {
        // Synchronous blocking inside async code.
        let _ = gate.recv();
        Ok(())
    });
    wait_for(&queue, id, TaskStatus::Running);
    let start = Instant::now();
    let stall = loop {
        if let Some(stall) = snapshot(&queue, id).stall {
            break stall;
        }
        assert!(start.elapsed() < TIMEOUT, "no stall reported");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(stall.kind, StallKind::BlockedPoll);
    queue.inner.watchdog_pass();
    assert!(log_text(&queue, id).contains("stalled: Inside one poll"));

    release.send(()).unwrap();
    wait_for(&queue, id, TaskStatus::Succeeded);
    assert!(snapshot(&queue, id).stall.is_none());
}

#[test]
fn a_task_waiting_with_no_activity_is_reported_pending() {
    let queue = TaskQueue::new();
    queue.set_stall_thresholds(quick_thresholds());
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    let id = queue.submit_async(desc(TaskDuration::Short), move |ctx| async move {
        ctx.set_phase("awaiting the other side");
        let _ = rx.await;
        Ok(())
    });
    let start = Instant::now();
    loop {
        let snap = snapshot(&queue, id);
        if let Some(stall) = snap.stall {
            assert_eq!(stall.kind, StallKind::Pending);
            assert!(stall.message.contains("awaiting the other side"));
            assert_eq!(snap.activity, TaskActivity::Pending);
            break;
        }
        assert!(start.elapsed() < TIMEOUT, "no pending stall");
        std::thread::sleep(Duration::from_millis(5));
    }
    tx.send(()).unwrap();
    wait_for(&queue, id, TaskStatus::Succeeded);
}

#[test]
fn a_future_nobody_polls_is_reported_as_never_polled() {
    let queue = TaskQueue::new();
    queue.set_stall_thresholds(quick_thresholds());
    let future = queue.track(desc(TaskDuration::Short), async {});
    let id = future.id();
    std::thread::sleep(Duration::from_millis(80));
    let stall = snapshot(&queue, id).stall.expect("stall");
    assert_eq!(stall.kind, StallKind::NeverPolled);
    drop(future);
}

#[test]
fn a_queued_task_behind_a_busy_pool_reports_the_queue_wait() {
    let (queue, _blocker, release) = queue_with_blocked_short_worker();
    queue.set_stall_thresholds(quick_thresholds());
    let waiting = queue.submit(desc(TaskDuration::Short), |_| Ok(()));
    std::thread::sleep(Duration::from_millis(80));
    let stall = snapshot(&queue, waiting).stall.expect("stall");
    assert_eq!(stall.kind, StallKind::QueueWait);
    assert!(
        stall.message.contains("1 task(s) are running"),
        "{}",
        stall.message
    );
    release.send(()).unwrap();
    wait_for(&queue, waiting, TaskStatus::Succeeded);
}

#[test]
fn log_cursor_reads_only_new_lines_and_counts_dropped() {
    let log = TaskLog::new();
    for i in 0..3 {
        log.push(format!("line {i}"));
    }
    let first = log.read_from(0);
    assert_eq!(first.lines.len(), 3);
    log.push("line 3".into());
    let next = log.read_from(first.next_cursor);
    assert_eq!(next.lines.len(), 1);
    assert!(next.lines[0].ends_with("line 3"));

    for i in 0..(MAX_LOG_LINES + 10) {
        log.push(format!("bulk {i}"));
    }
    let lagging = log.read_from(first.next_cursor);
    assert_eq!(lagging.lines.len(), MAX_LOG_LINES);
    assert!(lagging.dropped > 0);
}

#[test]
fn finished_history_is_bounded_and_keeps_starred_tasks() {
    let queue = TaskQueue::new();
    queue.scope(TaskDescription::foreground("keep", "test"), |_| ());
    let keep_id = queue.snapshots()[0].id;
    queue.set_starred(keep_id, true);
    for _ in 0..(MAX_FINISHED_TASKS + 20) {
        queue.scope(TaskDescription::foreground("bulk", "test"), |_| ());
    }
    let snapshots = queue.snapshots();
    assert!(snapshots.len() <= MAX_FINISHED_TASKS + 1);
    assert!(snapshots.iter().any(|s| s.id == keep_id));
}
