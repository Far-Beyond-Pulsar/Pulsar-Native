use gpui::*;

use crate::components::ProblemsDrawer;
use crate::screen::lock_diagnostics;
use crate::utils::types::{DiagnosticSeverity, DiffLineType};

pub fn compute_aligned_diff(
    before: &str,
    after: &str,
) -> (
    Vec<(Option<usize>, DiffLineType, String)>,
    Vec<(Option<usize>, DiffLineType, String)>,
) {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(before, after);

    let mut left: Vec<(Option<usize>, DiffLineType, String)> = Vec::new();
    let mut right: Vec<(Option<usize>, DiffLineType, String)> = Vec::new();
    let (mut ln, mut rn) = (1usize, 1usize);

    for change in diff.iter_all_changes() {
        let content = change.value().trim_end_matches('\n').to_string();
        match change.tag() {
            ChangeTag::Equal => {
                left.push((Some(ln), DiffLineType::Unchanged, content.clone()));
                right.push((Some(rn), DiffLineType::Unchanged, content));
                ln += 1;
                rn += 1;
            }
            ChangeTag::Delete => {
                left.push((Some(ln), DiffLineType::Deleted, content));
                right.push((None, DiffLineType::Spacer, String::new()));
                ln += 1;
            }
            ChangeTag::Insert => {
                left.push((None, DiffLineType::Spacer, String::new()));
                right.push((Some(rn), DiffLineType::Added, content));
                rn += 1;
            }
        }
    }
    (left, right)
}

impl ProblemsDrawer {
    pub fn count_by_severity(&self, severity: DiagnosticSeverity) -> usize {
        lock_diagnostics(&self.diagnostics)
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    }

    pub fn total_count(&self) -> usize {
        lock_diagnostics(&self.diagnostics).len()
    }

    pub(crate) fn set_filter(
        &mut self,
        severity: Option<DiagnosticSeverity>,
        cx: &mut Context<Self>,
    ) {
        self.filtered_severity = severity;
        self.refresh_rows();
        cx.notify();
    }

    pub(crate) fn set_source_filter(&mut self, source: Option<String>, cx: &mut Context<Self>) {
        self.filtered_source = source;
        self.refresh_rows();
        cx.notify();
    }
}

/// Path shown in the list: relative to the project root (prefixed with the
/// root directory's name) when inside it, otherwise the original path, always
/// with forward slashes.
pub(crate) fn display_path(project_root: Option<&std::path::Path>, absolute_path: &str) -> String {
    if let Some(project_root) = project_root {
        let abs = std::path::PathBuf::from(absolute_path);
        if let Ok(relative) = abs.strip_prefix(project_root) {
            if let Some(name) = project_root.file_name() {
                let mut p = std::path::PathBuf::from(name);
                p.push(relative);
                return p.to_string_lossy().replace('\\', "/");
            }
        }
    }
    absolute_path.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` (pulled in by `super::*`) exports a `test` macro that would shadow `#[test]`.
    use core::prelude::v1::test;
    use std::path::Path;

    #[test]
    fn display_path_is_relative_to_project_root_with_root_name() {
        let root = Path::new("/work/MyGame");
        assert_eq!(
            display_path(Some(root), "/work/MyGame/scripts/main.rs"),
            "MyGame/scripts/main.rs"
        );
    }

    #[test]
    fn display_path_outside_root_or_without_root_is_unchanged() {
        let root = Path::new("/work/MyGame");
        assert_eq!(display_path(Some(root), "/other/file.rs"), "/other/file.rs");
        assert_eq!(display_path(None, "C:\\proj\\a.rs"), "C:/proj/a.rs");
    }

    #[test]
    fn aligned_diff_pads_with_spacers_and_numbers_each_side() {
        let (left, right) = compute_aligned_diff("a\nb\nc\n", "a\nx\nc\nd\n");
        assert_eq!(left.len(), right.len());
        assert_eq!(left[0], (Some(1), DiffLineType::Unchanged, "a".to_string()));
        assert_eq!(
            right[0],
            (Some(1), DiffLineType::Unchanged, "a".to_string())
        );
        assert!(left.contains(&(Some(2), DiffLineType::Deleted, "b".to_string())));
        assert!(right.contains(&(Some(2), DiffLineType::Added, "x".to_string())));
        assert!(left.contains(&(None, DiffLineType::Spacer, String::new())));
        assert!(right.contains(&(Some(4), DiffLineType::Added, "d".to_string())));
    }

    #[test]
    fn aligned_diff_of_identical_text_has_no_changes() {
        let (left, right) = compute_aligned_diff("one\ntwo\n", "one\ntwo\n");
        assert!(left.iter().all(|l| l.1 == DiffLineType::Unchanged));
        assert_eq!(left, right);
        assert_eq!(left.len(), 2);
    }

    #[test]
    fn severity_labels_are_stable() {
        assert_eq!(DiagnosticSeverity::Error.label(), "Error");
        assert_eq!(DiagnosticSeverity::Warning.label(), "Warning");
        assert_eq!(DiagnosticSeverity::Information.label(), "Info");
        assert_eq!(DiagnosticSeverity::Hint.label(), "Hint");
    }
}
