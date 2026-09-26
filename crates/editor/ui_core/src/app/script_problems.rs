//! Script problems in the problems panel (Pulsar-Native#854, #868).
//!
//! The Play-in-Editor host publishes the running game's script problems
//! (runtime errors, link errors, dropped waiting calls: class, function and
//! graph node) on the process-wide host bus
//! (`pulsar_events::publish_script_problem`). This keeps the latest
//! session's in the problems drawer, under their own source, next to the
//! language server's diagnostics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{Context, Entity};
use pulsar_events::{ProblemSeverity, ScriptProblem, ScriptProblemsEvent};
use ui_problems::{Diagnostic, DiagnosticSeverity, ProblemsDrawer};

/// The drawer's source name for these diagnostics.
const SOURCE: &str = "Scripts (Play)";
/// Problems the panel shows at most.
const MAX_SHOWN: usize = 200;

#[derive(Default)]
struct Shared {
    problems: Mutex<Vec<ScriptProblem>>,
    version: AtomicU64,
}

/// Follow script problems into `drawer` for the app's lifetime.
pub(crate) fn watch<T: 'static>(drawer: Entity<ProblemsDrawer>, cx: &mut Context<T>) {
    let shared = Arc::new(Shared::default());
    let sink = Arc::clone(&shared);
    // Delivered on the publisher's thread: only record.
    let subscription = pulsar_events::subscribe_script_problems(move |event| {
        let mut problems = sink.problems.lock().unwrap_or_else(|p| p.into_inner());
        match event {
            ScriptProblemsEvent::Reported(problem) => {
                // The same failure repeats every frame: keep one.
                if !problems.contains(problem) {
                    problems.push(problem.clone());
                    let excess = problems.len().saturating_sub(MAX_SHOWN);
                    problems.drain(..excess);
                }
            }
            ScriptProblemsEvent::Cleared => problems.clear(),
        }
        sink.version.fetch_add(1, Ordering::Release);
    });
    let drawer = drawer.downgrade();
    cx.spawn(async move |_, cx| {
        let _subscription = subscription;
        let mut seen = 0;
        loop {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            let version = shared.version.load(Ordering::Acquire);
            if version == seen {
                continue;
            }
            seen = version;
            let diagnostics: Vec<Diagnostic> = shared
                .problems
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .map(diagnostic)
                .collect();
            if drawer
                .update(cx, |drawer, cx| drawer.set_external_diagnostics(SOURCE, diagnostics, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

fn diagnostic(problem: &ScriptProblem) -> Diagnostic {
    Diagnostic {
        file_path: problem
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .or_else(|| problem.class.clone())
            .unwrap_or_default(),
        line: problem.line.unwrap_or(0) as usize,
        column: 0,
        end_line: None,
        end_column: None,
        severity: match problem.severity {
            ProblemSeverity::Error => DiagnosticSeverity::Error,
            ProblemSeverity::Warning => DiagnosticSeverity::Warning,
        },
        // Class, function and node, then the error.
        message: problem.summary(),
        source: Some(SOURCE.to_owned()),
        hints: Vec::new(),
        subitems: Vec::new(),
        loading_actions: false,
    }
}
