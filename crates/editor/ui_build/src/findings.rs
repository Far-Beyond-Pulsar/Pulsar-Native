//! What a build or a game run tells the Problems panel.
//!
//! A failed build and a game that did not start or crashed are facts a person
//! acts on, so each is a finding (`pulsar_findings`), not only a notification
//! that goes away: it stays in the Project tab until the next build or run
//! replaces it. Two sources, `Build` and `Game`; each publishes its complete
//! set, so starting the next build or run clears the last one's.

use std::path::Path;

use pulsar_findings::{Action, Category, Finding, Severity};
use rust_i18n::t;

const BUILD_SOURCE: &str = "Build";
const GAME_SOURCE: &str = "Game";

/// How much of an error text a finding keeps; the rest is in the build's
/// task log and the failure dialog.
const DETAIL_KEEP: usize = 1_500;

fn tail(text: &str) -> &str {
    let text = text.trim();
    let mut start = text.len().saturating_sub(DETAIL_KEEP);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn head(text: &str) -> &str {
    let text = text.trim();
    let mut end = text.len().min(DETAIL_KEEP);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A build is starting: the last one's failure no longer stands.
pub(crate) fn clear_build() {
    pulsar_findings::publish(BUILD_SOURCE, Vec::new());
}

/// A game run is starting: the last one's failure no longer stands.
pub(crate) fn clear_game() {
    pulsar_findings::publish(GAME_SOURCE, Vec::new());
}

/// The build `name` failed with `message` (the compiler errors, or what the
/// pack step refused).
pub(crate) fn build_failed(name: &str, message: &str) {
    pulsar_findings::publish(
        BUILD_SOURCE,
        vec![
            Finding::new(
                "build-failed",
                Category::Project,
                Severity::Error,
                t!("Build.Finding.BuildFailed", name = name).to_string(),
            )
            .detail(head(message))
            .subject(t!("Build.Finding.BuildSubject").to_string())
            .action(
                t!("Build.Finding.OpenTasks").to_string(),
                Action::OpenWindow {
                    name: "TasksWindow",
                },
            ),
        ],
    );
}

/// The game could not be started.
pub(crate) fn game_launch_failed(project_root: &Path, error: &str) {
    pulsar_findings::publish(
        GAME_SOURCE,
        vec![
            Finding::new(
                "game-launch-failed",
                Category::Project,
                Severity::Error,
                t!("Build.Finding.LaunchFailed").to_string(),
            )
            .detail(format!("{error}\n{}", project_root.display()))
            .subject(t!("Build.Finding.GameSubject").to_string()),
        ],
    );
}

/// The game ran and exited with a failure; `stderr` is what it wrote and
/// `report` the crash report saved from it.
pub(crate) fn game_crashed(stderr: &str, report: Option<&Path>) {
    let mut finding = Finding::new(
        "game-crashed",
        Category::Project,
        Severity::Error,
        t!("Build.Finding.Crashed").to_string(),
    )
    .detail(if stderr.trim().is_empty() {
        t!("Build.Game.NonZeroExit").to_string()
    } else {
        tail(stderr).to_owned()
    })
    .subject(t!("Build.Finding.GameSubject").to_string());
    if let Some(report) = report {
        finding = finding.action(
            t!("Build.Finding.OpenReport").to_string(),
            Action::OpenFile {
                path: report.to_path_buf(),
                line: 1,
                column: 1,
            },
        );
    }
    pulsar_findings::publish(GAME_SOURCE, vec![finding]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        pulsar_findings::snapshot()
            .values()
            .flat_map(|set| set.iter().map(|f| f.key.clone()))
            .collect()
    }

    #[test]
    fn a_failed_build_and_a_crash_are_findings_until_the_next_run_clears_them() {
        build_failed("Build & Run", "pack: script errors:\n  error [A] x");
        game_crashed("thread 'main' panicked", None);
        let all = keys();
        assert!(all.contains(&"build-failed".to_owned()), "{all:?}");
        assert!(all.contains(&"game-crashed".to_owned()), "{all:?}");
        clear_build();
        clear_game();
        let left = keys();
        assert!(!left.contains(&"build-failed".to_owned()), "{left:?}");
        assert!(!left.contains(&"game-crashed".to_owned()), "{left:?}");
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let long = "é".repeat(DETAIL_KEEP);
        assert!(head(&long).len() <= DETAIL_KEEP);
        assert!(tail(&long).len() <= DETAIL_KEEP);
    }
}
