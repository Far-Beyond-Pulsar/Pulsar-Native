//! Editor health: tasks that look stalled now, and recent failures (an
//! unresponsive UI thread and slow UI work are recorded as failed tasks by
//! `editor_task_queue::ui_watch`).

use std::time::{Duration, SystemTime};

use editor_task_queue::{TaskSnapshot, TaskStatus};
use pulsar_findings::{Action, Category, Finding, Severity};

pub(super) const SOURCE: &str = "Editor tasks";
/// Failures older than this are history, not something to act on.
const RECENT: Duration = Duration::from_secs(10 * 60);
/// The most recent failures listed.
const MAX_FAILURES: usize = 20;

pub(super) fn collect() -> Vec<Finding> {
    findings(&editor_task_queue::global().snapshots(), SystemTime::now())
}

fn tasks_window() -> Action {
    Action::OpenWindow {
        name: "EditorTasksWindow",
    }
}

/// The findings of `tasks` at `now`.
fn findings(tasks: &[TaskSnapshot], now: SystemTime) -> Vec<Finding> {
    let mut out = Vec::new();
    for task in tasks {
        if !matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
            continue;
        }
        let Some(stall) = &task.stall else {
            continue;
        };
        out.push(
            Finding::new(
                format!("stall:{}:{}", task.title, task.origin),
                Category::Editor,
                Severity::Warning,
                format!("{} looks stalled", task.title),
            )
            .detail(format!("{}\nStarted at {}.", stall.message, task.origin))
            .subject("Stalled tasks")
            .metric(format!("{}s", task.idle_for.as_secs()))
            .action("Open Tasks", tasks_window()),
        );
    }

    let mut failures: Vec<&TaskSnapshot> = tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Failed)
        .filter(|task| {
            task.finished_at
                .and_then(|at| now.duration_since(at).ok())
                .is_some_and(|age| age <= RECENT)
        })
        .collect();
    failures.sort_by_key(|task| std::cmp::Reverse(task.finished_at));
    for task in failures.into_iter().take(MAX_FAILURES) {
        // The UI watcher's tasks are about responsiveness; the rest are
        // work that failed.
        let responsiveness = task.title.starts_with("UI thread unresponsive")
            || task.title.starts_with("Slow UI work");
        let subject = if responsiveness {
            "Responsiveness"
        } else {
            "Failed tasks"
        };
        out.push(
            Finding::new(
                format!("failed:{}:{}", task.title, task.origin),
                Category::Editor,
                if responsiveness {
                    Severity::Warning
                } else {
                    Severity::Error
                },
                task.title.clone(),
            )
            .detail(task.error.clone().unwrap_or_default())
            .subject(subject)
            .action("Open Tasks", tasks_window()),
        );
    }
    out
}
