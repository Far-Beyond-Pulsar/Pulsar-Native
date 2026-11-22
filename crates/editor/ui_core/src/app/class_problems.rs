//! Class problems in the problems panel: the one-language-per-class rule
//! (`pulsar_class::ClassProblem`).
//!
//! The host owns the rule, so the panel shows a class that is two languages'
//! (or whose module another language wrote) whichever languages are enabled.
//! The project's classes are scanned on a worker, once when the project
//! opens, when the plugin set changes (enabling or disabling a language
//! changes which markers count), within a tick of a change to the project's
//! files (`engine_fs::events`: a class folder created, a module written), and
//! every half minute for anything the events miss, under their own source.

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
    let mut files = engine_fs::events::subscribe();
    cx.spawn(async move |_, cx| {
        let mut generation = None;
        let mut ticks = RESCAN_TICKS;
        loop {
            let now = pulsar_plugins::generation();
            let changed = project_files_changed(&mut files, &project);
            if generation != Some(now) || changed || ticks >= RESCAN_TICKS {
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

/// Whether any project file changed since the last tick (build and VCS folders
/// aside). Takes every queued event, so a burst (a save, a checkout) is one
/// rescan; a lagged receiver dropped events, which counts as a change.
fn project_files_changed(
    files: &mut tokio::sync::broadcast::Receiver<engine_fs::events::FsEvent>,
    project: &std::path::Path,
) -> bool {
    use tokio::sync::broadcast::error::TryRecvError;
    let mut changed = false;
    loop {
        match files.try_recv() {
            Ok(event) => {
                changed |= event.path.starts_with(project)
                    && !engine_fs::file_index::is_unindexed_path(project, &event.path);
            }
            Err(TryRecvError::Lagged(_)) => changed = true,
            Err(TryRecvError::Empty | TryRecvError::Closed) => return changed,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use engine_fs::events::{FsChangeKind, FsEvent, FsEventSource};
    use std::path::Path;
    use tokio::sync::broadcast;

    fn event(path: &str) -> FsEvent {
        FsEvent {
            path: PathBuf::from(path),
            kind: FsChangeKind::Modified,
            source: FsEventSource::Local,
        }
    }

    #[test]
    fn a_change_inside_the_project_triggers_a_rescan_once() {
        let project = Path::new("/proj");
        let (tx, mut rx) = broadcast::channel(8);
        assert!(!project_files_changed(&mut rx, project), "nothing queued");

        tx.send(event("/elsewhere/a.json")).unwrap();
        tx.send(event("/proj/target/debug/x")).unwrap();
        assert!(!project_files_changed(&mut rx, project), "outside, or a build folder");

        tx.send(event("/proj/src/classes/Door.class/graph.json")).unwrap();
        tx.send(event("/proj/src/classes/Door.class/prefab.json")).unwrap();
        assert!(project_files_changed(&mut rx, project));
        assert!(!project_files_changed(&mut rx, project), "the burst was taken");
    }

    #[test]
    fn dropped_events_count_as_a_change() {
        let (tx, mut rx) = broadcast::channel(1);
        tx.send(event("/elsewhere/a")).unwrap();
        tx.send(event("/elsewhere/b")).unwrap();
        assert!(project_files_changed(&mut rx, Path::new("/proj")));
    }
}
