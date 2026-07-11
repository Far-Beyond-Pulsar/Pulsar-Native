use std::collections::HashMap;

use gpui::*;

use crate::components::ProblemsDrawer;
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
        let diagnostics = self.diagnostics.lock().unwrap();

        if self.filtered_severity.is_none() && self.search_query.is_empty() {
            return diagnostics.clone();
        }

        let query = if !self.search_query.is_empty() {
            Some(self.search_query.to_lowercase())
        } else {
            None
        };

        diagnostics
            .iter()
            .filter(|d| {
                if let Some(sev) = &self.filtered_severity {
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

    pub(crate) fn get_grouped_diagnostics(&self) -> HashMap<String, Vec<Diagnostic>> {
        let mut grouped: HashMap<String, Vec<Diagnostic>> = HashMap::new();
        for d in self.get_filtered_diagnostics() {
            grouped.entry(d.file_path.clone()).or_default().push(d);
        }
        grouped
    }

    pub fn count_by_severity(&self, severity: DiagnosticSeverity) -> usize {
        self.diagnostics
            .lock()
            .unwrap()
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    }

    pub fn total_count(&self) -> usize {
        self.diagnostics.lock().unwrap().len()
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
        if let Some(project_root) = &self.project_root {
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
}
