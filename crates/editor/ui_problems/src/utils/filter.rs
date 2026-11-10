use std::collections::HashMap;

use gpui::*;

use crate::components::ProblemsDrawer;
use crate::screen::lock_diagnostics;
use crate::utils::types::{Diagnostic, DiagnosticSeverity, DiffLineType};

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
    pub(crate) fn get_filtered_diagnostics(&self) -> Vec<Diagnostic> {
        let diagnostics = lock_diagnostics(&self.diagnostics);
        filter_diagnostics(
            &diagnostics,
            self.filtered_severity.as_ref(),
            &self.search_query,
        )
    }

    pub(crate) fn get_grouped_diagnostics(&self) -> HashMap<String, Vec<Diagnostic>> {
        group_by_file(self.get_filtered_diagnostics())
    }

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
        self.selected_index = None;
        cx.notify();
    }

    pub(crate) fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.selected_index = None;
        cx.notify();
    }

    pub(crate) fn get_display_path(&self, absolute_path: &str) -> String {
        display_path(self.project_root.as_deref(), absolute_path)
    }
}

/// Keep diagnostics matching `severity` (if any) and whose message, file path
/// or source contains `search_query` (case-insensitive; empty matches all).
fn filter_diagnostics(
    diagnostics: &[Diagnostic],
    severity: Option<&DiagnosticSeverity>,
    search_query: &str,
) -> Vec<Diagnostic> {
    if severity.is_none() && search_query.is_empty() {
        return diagnostics.to_vec();
    }

    let query = (!search_query.is_empty()).then(|| search_query.to_lowercase());

    diagnostics
        .iter()
        .filter(|d| {
            if let Some(sev) = severity {
                if &d.severity != sev {
                    return false;
                }
            }
            if let Some(q) = &query {
                return d.message.to_lowercase().contains(q)
                    || d.file_path.to_lowercase().contains(q)
                    || d.source
                        .as_ref()
                        .is_some_and(|s| s.to_lowercase().contains(q));
            }
            true
        })
        .cloned()
        .collect()
}

/// Group diagnostics by file path, preserving their relative order per file.
fn group_by_file(diagnostics: Vec<Diagnostic>) -> HashMap<String, Vec<Diagnostic>> {
    let mut grouped: HashMap<String, Vec<Diagnostic>> = HashMap::new();
    for d in diagnostics {
        grouped.entry(d.file_path.clone()).or_default().push(d);
    }
    grouped
}

/// Path shown in the list: relative to the project root (prefixed with the
/// root directory's name) when inside it, otherwise the original path, always
/// with forward slashes.
fn display_path(project_root: Option<&std::path::Path>, absolute_path: &str) -> String {
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

    fn diag(
        file: &str,
        severity: DiagnosticSeverity,
        message: &str,
        source: Option<&str>,
    ) -> Diagnostic {
        Diagnostic {
            file_path: file.into(),
            line: 1,
            column: 1,
            end_line: None,
            end_column: None,
            severity,
            message: message.into(),
            source: source.map(String::from),
            hints: Vec::new(),
            subitems: Vec::new(),
            loading_actions: false,
        }
    }

    fn sample() -> Vec<Diagnostic> {
        vec![
            diag(
                "src/a.rs",
                DiagnosticSeverity::Error,
                "mismatched types",
                Some("rustc"),
            ),
            diag(
                "src/a.rs",
                DiagnosticSeverity::Warning,
                "unused variable",
                Some("clippy"),
            ),
            diag(
                "src/b.rs",
                DiagnosticSeverity::Error,
                "cannot find value",
                None,
            ),
            diag(
                "src/Widget.rs",
                DiagnosticSeverity::Information,
                "note",
                None,
            ),
        ]
    }

    #[test]
    fn no_filter_returns_everything_in_order() {
        assert_eq!(filter_diagnostics(&sample(), None, ""), sample());
    }

    #[test]
    fn severity_filter_keeps_only_that_severity() {
        let out = filter_diagnostics(&sample(), Some(&DiagnosticSeverity::Error), "");
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|d| d.severity == DiagnosticSeverity::Error));
    }

    #[test]
    fn search_matches_message_path_and_source_case_insensitively() {
        let by_message = filter_diagnostics(&sample(), None, "UNUSED");
        assert_eq!(by_message.len(), 1);
        assert_eq!(by_message[0].message, "unused variable");

        let by_path = filter_diagnostics(&sample(), None, "widget");
        assert_eq!(by_path.len(), 1);
        assert_eq!(by_path[0].file_path, "src/Widget.rs");

        let by_source = filter_diagnostics(&sample(), None, "Clippy");
        assert_eq!(by_source.len(), 1);
        assert_eq!(by_source[0].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn severity_and_search_are_combined_with_and() {
        let out = filter_diagnostics(&sample(), Some(&DiagnosticSeverity::Error), "cannot");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].file_path, "src/b.rs");
        assert!(
            filter_diagnostics(&sample(), Some(&DiagnosticSeverity::Warning), "cannot").is_empty()
        );
    }

    #[test]
    fn group_by_file_preserves_order_within_each_file() {
        let grouped = group_by_file(sample());
        assert_eq!(grouped.len(), 3);
        let a = &grouped["src/a.rs"];
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].message, "mismatched types");
        assert_eq!(a[1].message, "unused variable");
    }

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
