//! Class problems in the problems panel: the one-language-per-class rule
//! (`pulsar_class::ClassProblem`).
//!
//! The host owns the rule, so the panel shows a class that is two languages'
//! (or whose module another language wrote) whichever languages are enabled.
//! The project's classes are scanned on a worker, once when the project
//! opens, when the plugin set changes (enabling or disabling a language
//! changes which markers count) and then every half minute, under their own
//! source.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{Context, Entity};
use ui_problems::{Diagnostic, DiagnosticSeverity, ProblemsDrawer};

/// The drawer's source name for these diagnostics.
const SOURCE: &str = "Classes";
/// How often the project is looked at.
const TICK: Duration = Duration::from_secs(2);
/// How often the classes are rescanned when nothing else changed.
const RESCAN_TICKS: u32 = 15;

/// Follow the class problems of `project` into `drawer` for the app's
/// lifetime. A window with no project has none.
pub(crate) fn watch<T: 'static>(
    drawer: Entity<ProblemsDrawer>,
    project: Option<PathBuf>,
    cx: &mut Context<T>,
) {
    let Some(project) = project else {
        return;
    };
    let drawer = drawer.downgrade();
    cx.spawn(async move |_, cx| {
        let mut generation = None;
        let mut ticks = RESCAN_TICKS;
        loop {
            let now = pulsar_plugins::generation();
            if generation != Some(now) || ticks >= RESCAN_TICKS {
                generation = Some(now);
                ticks = 0;
                let root = project.clone();
                let problems = cx
                    .background_executor()
                    .spawn(async move { pulsar_class::class_problems(&root) })
                    .await;
                let diagnostics: Vec<Diagnostic> = problems.iter().map(diagnostic).collect();
                if drawer
                    .update(cx, |drawer, cx| {
                        drawer.set_source_diagnostics(SOURCE, diagnostics, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
            cx.background_executor().timer(TICK).await;
            ticks += 1;
        }
    })
    .detach();
}

fn diagnostic(problem: &pulsar_class::ClassProblem) -> Diagnostic {
    Diagnostic {
        file_path: problem.file().display().to_string(),
        line: 0,
        column: 0,
        end_line: None,
        end_column: None,
        severity: DiagnosticSeverity::Error,
        message: problem.to_string(),
        source: Some(SOURCE.to_owned()),
        hints: Vec::new(),
        subitems: Vec::new(),
        loading_actions: false,
    }
}
