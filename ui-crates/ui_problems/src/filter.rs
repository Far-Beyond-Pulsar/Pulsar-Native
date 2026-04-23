//! Diagnostic type definitions and filter logic for the Problems panel.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::*;
use ui::ActiveTheme as _;
use ui::IconName;

use crate::problems_drawer::ProblemsDrawer;

// ── Diff helpers ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLineType {
    Unchanged,
    Added,
    Deleted,
    Spacer,
}

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

// ── Core diagnostic types ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub end_line: Option<usize>,
    pub end_column: Option<usize>,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
    pub hints: Vec<Hint>,
    pub subitems: Vec<Diagnostic>,
    pub loading_actions: bool,
}

#[derive(Clone, Debug)]
pub struct Hint {
    pub message: String,
    pub before_content: Option<String>,
    pub after_content: Option<String>,
    pub file_path: Option<String>,
    pub line: Option<usize>,
    pub loading: bool,
}

#[derive(Clone, Debug)]
pub struct NavigateToDiagnostic {
    pub file_path: PathBuf,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl DiagnosticSeverity {
    pub fn icon(&self) -> IconName {
        match self {
            Self::Error => IconName::Close,
            Self::Warning => IconName::TriangleAlert,
            Self::Information => IconName::Info,
            Self::Hint => IconName::Info,
        }
    }

    pub fn color(&self, cx: &App) -> Hsla {
        match self {
            Self::Error => Hsla {
                h: 0.0,
                s: 0.85,
                l: 0.55,
                a: 1.0,
            },
            Self::Warning => Hsla {
                h: 38.0,
                s: 0.95,
                l: 0.55,
                a: 1.0,
            },
            Self::Information => Hsla {
                h: 210.0,
                s: 0.80,
                l: 0.60,
                a: 1.0,
            },
            Self::Hint => cx.theme().muted_foreground,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Information => "Info",
            Self::Hint => "Hint",
        }
    }
}

// ── Filter methods on ProblemsDrawer ─────────────────────────────────────────

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
