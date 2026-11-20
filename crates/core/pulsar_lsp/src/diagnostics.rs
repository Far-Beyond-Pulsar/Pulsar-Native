//! Diagnostics from every source, in one place: file by source.
//!
//! A language server's `publishDiagnostics`, a compiler's output and a play
//! session's errors all land in the [`DiagnosticsStore`]. A source publishes
//! the complete set for a file; an empty set clears it (so a fixed error
//! goes), and a server that stops takes its entries with it. Editors,
//! the Problems drawer, the footer counts and the AI tools read the store;
//! none keeps a copy.
//!
//! The data lives in [`DiagnosticsData`] (no GPUI, so its rules are tested
//! directly); [`DiagnosticsStore`] is the entity around it that emits
//! [`DiagnosticsEvent`]s.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{Context, EventEmitter};
use serde_json::Value;
use ui::diagnostics::{Diagnostic, DiagnosticSeverity};

use crate::code_actions;
use crate::path_utils::{path_key, uri_to_path};
use pulsar_language::spec::ServerHooks;

/// One running server: a spec in a root.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerId {
    pub server: &'static str,
    pub root: PathBuf,
}

impl std::fmt::Display for ServerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.server, self.root.display())
    }
}

/// Where a diagnostic came from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticSource {
    /// A language server's `publishDiagnostics`.
    Server(ServerId),
    /// A script language's compiler (`typescript`).
    Compiler(&'static str),
}

impl DiagnosticSource {
    /// What the drawer and hovers call it: the server's or compiler's id.
    pub fn label(&self) -> String {
        match self {
            Self::Server(id) => id.server.to_string(),
            Self::Compiler(language) => format!("{language} compiler"),
        }
    }
}

/// Totals for the footer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub hints: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.errors + self.warnings + self.infos + self.hints
    }
}

struct FileEntry {
    path: PathBuf,
    by_source: Vec<(DiagnosticSource, Vec<Diagnostic>)>,
}

/// The store's data and rules, without GPUI.
#[derive(Default)]
pub struct DiagnosticsData {
    files: HashMap<String, FileEntry>,
}

impl DiagnosticsData {
    /// Replace `source`'s diagnostics for `path` (empty clears). Returns
    /// whether anything changed.
    pub fn set(&mut self, source: DiagnosticSource, path: &Path, diagnostics: Vec<Diagnostic>) -> bool {
        let key = path_key(path);
        if diagnostics.is_empty() {
            let Some(entry) = self.files.get_mut(&key) else {
                return false;
            };
            let before = entry.by_source.len();
            entry.by_source.retain(|(existing, _)| *existing != source);
            let changed = entry.by_source.len() != before;
            if entry.by_source.is_empty() {
                self.files.remove(&key);
            }
            return changed;
        }
        let entry = self.files.entry(key).or_insert_with(|| FileEntry {
            path: path.to_path_buf(),
            by_source: Vec::new(),
        });
        match entry.by_source.iter_mut().find(|(existing, _)| *existing == source) {
            Some((_, current)) => *current = diagnostics,
            None => entry.by_source.push((source, diagnostics)),
        }
        true
    }

    /// Remove everything `matches` selects; returns the files that changed.
    fn clear_where(&mut self, matches: impl Fn(&DiagnosticSource) -> bool) -> Vec<(PathBuf, DiagnosticSource)> {
        let mut changed = Vec::new();
        for entry in self.files.values_mut() {
            for (source, _) in entry.by_source.iter().filter(|(source, _)| matches(source)) {
                changed.push((entry.path.clone(), source.clone()));
            }
            entry.by_source.retain(|(source, _)| !matches(source));
        }
        self.files.retain(|_, entry| !entry.by_source.is_empty());
        changed
    }

    pub fn clear_source(&mut self, source: &DiagnosticSource) -> Vec<(PathBuf, DiagnosticSource)> {
        self.clear_where(|existing| existing == source)
    }

    pub fn clear_server(&mut self, server: &ServerId) -> Vec<(PathBuf, DiagnosticSource)> {
        self.clear_where(|existing| matches!(existing, DiagnosticSource::Server(id) if id == server))
    }

    /// Every server of spec `spec` (any root).
    pub fn clear_spec(&mut self, spec: &str) -> Vec<(PathBuf, DiagnosticSource)> {
        self.clear_where(|existing| matches!(existing, DiagnosticSource::Server(id) if id.server == spec))
    }

    pub fn for_file(&self, path: &Path) -> Vec<(DiagnosticSource, Diagnostic)> {
        self.files
            .get(&path_key(path))
            .map(|entry| {
                entry
                    .by_source
                    .iter()
                    .flat_map(|(source, list)| list.iter().map(|d| (source.clone(), d.clone())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every entry, files in path order.
    pub fn all(&self) -> Vec<(PathBuf, DiagnosticSource, Diagnostic)> {
        let mut files: Vec<&FileEntry> = self.files.values().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
            .into_iter()
            .flat_map(|entry| {
                entry.by_source.iter().flat_map(|(source, list)| {
                    list.iter()
                        .map(|d| (entry.path.clone(), source.clone(), d.clone()))
                })
            })
            .collect()
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for entry in self.files.values() {
            for (_, list) in &entry.by_source {
                for diagnostic in list {
                    match diagnostic.severity {
                        DiagnosticSeverity::Error => counts.errors += 1,
                        DiagnosticSeverity::Warning => counts.warnings += 1,
                        DiagnosticSeverity::Information => counts.infos += 1,
                        DiagnosticSeverity::Hint => counts.hints += 1,
                    }
                }
            }
        }
        counts
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

/// What the store tells its subscribers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticsEvent {
    /// `source`'s diagnostics for the file `key` ([`path_key`]) changed
    /// (including to none).
    Changed {
        key: String,
        path: PathBuf,
        source: DiagnosticSource,
    },
}

/// The entity editors, the drawer and the footer subscribe to.
#[derive(Default)]
pub struct DiagnosticsStore {
    data: DiagnosticsData,
}

impl EventEmitter<DiagnosticsEvent> for DiagnosticsStore {}

impl DiagnosticsStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// `source`'s complete set for `path`; empty clears it.
    pub fn set(
        &mut self,
        source: DiagnosticSource,
        path: &Path,
        diagnostics: Vec<Diagnostic>,
        cx: &mut Context<Self>,
    ) {
        if self.data.set(source.clone(), path, diagnostics) {
            cx.emit(DiagnosticsEvent::Changed {
                key: path_key(path),
                path: path.to_path_buf(),
                source,
            });
            cx.notify();
        }
    }

    pub fn clear_source(&mut self, source: &DiagnosticSource, cx: &mut Context<Self>) {
        let changed = self.data.clear_source(source);
        self.emit_all(changed, cx);
    }

    /// A server stopped: its entries go, and every file it had diagnostics for
    /// is announced so editors and the drawer clear.
    pub fn clear_server(&mut self, server: &ServerId, cx: &mut Context<Self>) {
        let changed = self.data.clear_server(server);
        self.emit_all(changed, cx);
    }

    pub fn clear_spec(&mut self, spec: &str, cx: &mut Context<Self>) {
        let changed = self.data.clear_spec(spec);
        self.emit_all(changed, cx);
    }

    fn emit_all(&mut self, changed: Vec<(PathBuf, DiagnosticSource)>, cx: &mut Context<Self>) {
        if changed.is_empty() {
            return;
        }
        for (path, source) in changed {
            cx.emit(DiagnosticsEvent::Changed {
                key: path_key(&path),
                path,
                source,
            });
        }
        cx.notify();
    }

    pub fn for_file(&self, path: &Path) -> Vec<(DiagnosticSource, Diagnostic)> {
        self.data.for_file(path)
    }

    pub fn all(&self) -> Vec<(PathBuf, DiagnosticSource, Diagnostic)> {
        self.data.all()
    }

    pub fn counts(&self) -> Counts {
        self.data.counts()
    }

    pub fn file_count(&self) -> usize {
        self.data.file_count()
    }
}

/// A `textDocument/publishDiagnostics` notification's parameters, parsed:
/// the file and its complete diagnostic set. The path is decoded from the
/// URI properly (a percent-encoded drive letter, a Unix path's leading
/// slash).
pub fn parse_publish(params: &Value, server_id: &str, hooks: &ServerHooks) -> Option<(PathBuf, Vec<Diagnostic>)> {
    let uri = params.get("uri")?.as_str()?;
    let list = params.get("diagnostics")?.as_array()?;
    let path = uri_to_path(uri);
    let file = path.to_string_lossy().into_owned();
    let diagnostics = list
        .iter()
        .filter_map(|raw| {
            let mut diagnostic = parse_diagnostic(raw, &file, server_id)?;
            let fixes = (hooks.quick_fixes)(raw, &file);
            if !fixes.is_empty() {
                diagnostic.code_actions =
                    fixes.into_iter().map(code_actions::code_action_of).collect();
            }
            Some(diagnostic)
        })
        .collect();
    Some((path, diagnostics))
}

/// One LSP diagnostic (`range` 0-based; `severity` defaults to error when
/// the server omits it, as the spec says).
pub fn parse_diagnostic(raw: &Value, file_path: &str, server_id: &str) -> Option<Diagnostic> {
    let range = raw.get("range")?;
    let message = raw.get("message")?.as_str()?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as usize + 1;
    let column = start.get("character")?.as_u64()? as usize + 1;
    let (end_line, end_column) = range.get("end").map_or((None, None), |end| {
        (
            end.get("line").and_then(Value::as_u64).map(|l| l as usize + 1),
            end.get("character").and_then(Value::as_u64).map(|c| c as usize + 1),
        )
    });
    let severity = match raw.get("severity").and_then(Value::as_u64).unwrap_or(1) {
        1 => DiagnosticSeverity::Error,
        2 => DiagnosticSeverity::Warning,
        3 => DiagnosticSeverity::Information,
        4 => DiagnosticSeverity::Hint,
        _ => DiagnosticSeverity::Information,
    };
    let code = raw.get("code").and_then(|c| match c {
        Value::String(text) => Some(text.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    let source = raw
        .get("source")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| server_id.to_string());
    Some(Diagnostic {
        file_path: file_path.to_string(),
        line,
        column,
        end_line,
        end_column,
        severity,
        message: message.to_string(),
        code,
        source: Some(source),
        code_actions: Vec::new(),
        raw_lsp_diagnostic: Some(raw.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diag(severity: DiagnosticSeverity, message: &str) -> Diagnostic {
        Diagnostic {
            file_path: String::new(),
            line: 1,
            column: 1,
            end_line: None,
            end_column: None,
            severity,
            message: message.into(),
            code: None,
            source: None,
            code_actions: Vec::new(),
            raw_lsp_diagnostic: None,
        }
    }

    fn server(name: &'static str) -> DiagnosticSource {
        DiagnosticSource::Server(ServerId {
            server: name,
            root: PathBuf::from("/p"),
        })
    }

    #[test]
    fn an_empty_set_clears_the_file() {
        let mut data = DiagnosticsData::default();
        let path = Path::new("/p/a.rs");
        assert!(data.set(server("ra"), path, vec![diag(DiagnosticSeverity::Error, "x")]));
        assert_eq!(data.for_file(path).len(), 1);
        assert!(data.set(server("ra"), path, Vec::new()), "clearing is a change");
        assert!(data.for_file(path).is_empty());
        assert_eq!(data.file_count(), 0);
        assert!(!data.set(server("ra"), path, Vec::new()), "clearing nothing is not");
    }

    #[test]
    fn two_files_are_held_at_once_and_a_fixed_one_goes_alone() {
        // The Problems drawer showed only the last file published (finding 3).
        let mut data = DiagnosticsData::default();
        data.set(server("ra"), Path::new("/p/a.rs"), vec![diag(DiagnosticSeverity::Error, "a")]);
        data.set(server("ra"), Path::new("/p/b.rs"), vec![diag(DiagnosticSeverity::Warning, "b")]);
        assert_eq!(data.all().len(), 2);
        data.set(server("ra"), Path::new("/p/a.rs"), Vec::new());
        let all = data.all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].2.message, "b");
        assert_eq!(data.counts(), Counts { errors: 0, warnings: 1, infos: 0, hints: 0 });
    }

    #[test]
    fn sources_of_one_file_merge_and_replace_independently() {
        let mut data = DiagnosticsData::default();
        let path = Path::new("/p/a.ts");
        data.set(server("tsls"), path, vec![diag(DiagnosticSeverity::Error, "ts")]);
        data.set(DiagnosticSource::Compiler("typescript"), path, vec![diag(DiagnosticSeverity::Error, "pulsar")]);
        let merged = data.for_file(path);
        assert_eq!(merged.len(), 2);
        data.set(DiagnosticSource::Compiler("typescript"), path, vec![diag(DiagnosticSeverity::Warning, "pulsar2")]);
        let merged = data.for_file(path);
        assert_eq!(merged.len(), 2, "replaced, not appended");
        assert!(merged.iter().any(|(s, d)| matches!(s, DiagnosticSource::Compiler(_)) && d.message == "pulsar2"));
        assert!(merged.iter().any(|(s, d)| *s == server("tsls") && d.message == "ts"));
    }

    #[test]
    fn a_stopped_server_takes_its_entries_and_only_its_own() {
        let mut data = DiagnosticsData::default();
        data.set(server("ra"), Path::new("/p/a.rs"), vec![diag(DiagnosticSeverity::Error, "a")]);
        data.set(server("ra"), Path::new("/p/b.rs"), vec![diag(DiagnosticSeverity::Error, "b")]);
        data.set(DiagnosticSource::Compiler("typescript"), Path::new("/p/b.rs"), vec![diag(DiagnosticSeverity::Error, "c")]);
        let changed = data.clear_server(&ServerId { server: "ra", root: PathBuf::from("/p") });
        assert_eq!(changed.len(), 2, "every file it had is announced");
        assert_eq!(data.all().len(), 1);
        assert!(matches!(data.all()[0].1, DiagnosticSource::Compiler("typescript")));
        // Another root of the same server is another server.
        data.set(
            DiagnosticSource::Server(ServerId { server: "ra", root: PathBuf::from("/q") }),
            Path::new("/q/x.rs"),
            vec![diag(DiagnosticSeverity::Hint, "q")],
        );
        data.clear_server(&ServerId { server: "ra", root: PathBuf::from("/p") });
        assert_eq!(data.all().len(), 2);
        assert_eq!(data.clear_spec("ra").len(), 1);
    }

    #[test]
    fn the_published_path_is_decoded_from_the_uri() {
        // Finding 4: `file:///c%3A/...` and Unix paths were mangled.
        let params = json!({
            "uri": "file:///c%3A/My%20Project/src/a.rs",
            "diagnostics": [{
                "range": { "start": { "line": 2, "character": 4 }, "end": { "line": 2, "character": 9 } },
                "message": "bad", "code": "E0308"
            }]
        });
        let (path, list) = parse_publish(&params, "a-server", &ServerHooks::DEFAULT).unwrap();
        assert_eq!(path, PathBuf::from("c:/My Project/src/a.rs"));
        assert_eq!(list[0].file_path, path.to_string_lossy());
        assert_eq!((list[0].line, list[0].column), (3, 5));
        assert_eq!(list[0].severity, DiagnosticSeverity::Error, "missing severity is an error");
        assert_eq!(list[0].source.as_deref(), Some("a-server"));
        let unix = json!({ "uri": "file:///home/u/a.rs", "diagnostics": [] });
        assert_eq!(
            parse_publish(&unix, "x", &ServerHooks::DEFAULT).unwrap().0,
            PathBuf::from("/home/u/a.rs")
        );
    }
}
