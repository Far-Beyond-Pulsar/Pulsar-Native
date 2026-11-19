//! Live compiler checks: a script language's own diagnostics for an open
//! file's unsaved text, published beside the language server's.
//!
//! A language server accepts constructs the engine's compiler rejects (the
//! TypeScript server takes imports, arrow functions and generics that the
//! script subset does not), so a file can be green in the editor and red at
//! Play unless the compiler's verdict reaches the editor too. The editor
//! hands the service the text on open, on edit and on save
//! ([`LanguageServers::check_source`]); the service debounces per file, runs
//! the language's [`ScriptLanguage::check_source`] as an editor task (one in
//! flight per file, the latest text wins) and publishes the result under
//! [`DiagnosticSource::Compiler`], with the source name `Pulsar`.
//!
//! The service also keeps the compiler source true: a language whose plugin is
//! disabled loses its entries, and a language whose own setting turns the
//! live check off ([`ScriptLanguage::live_check_enabled`]) loses them until it
//! is on again, when the open files are checked again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, Task};
use pulsar_language::{
    CheckContext, CompileDiagnostic, DiagnosticSeverity as CompileSeverity, Language,
    LanguageRegistry, NativeRegistry, ScriptLanguage,
};
use ui::diagnostics::{Diagnostic, DiagnosticSeverity};

use super::LanguageServers;
use crate::diagnostics::DiagnosticSource;
use crate::path_utils::path_key;

/// The name compiler diagnostics carry: what the drawer and a hover show.
pub const COMPILER_SOURCE: &str = "Pulsar";

/// How long after the last edit a file is checked.
pub const DEBOUNCE: Duration = Duration::from_millis(400);

/// What the service knows about one file it checks.
struct FileCheck {
    path: PathBuf,
    language: &'static Language,
    text: Arc<str>,
    /// Bumped by every request; the latest wins.
    version: u64,
    /// The debounce for `version` has elapsed.
    due: bool,
    running: bool,
    /// The version whose result was published (or dropped as superseded).
    done: u64,
    /// The entries were removed because the language's setting turned the
    /// check off; the file is checked again when it turns on.
    suspended: bool,
    _timer: Option<Task<()>>,
}

#[derive(Default)]
pub(super) struct Checks {
    files: HashMap<String, FileCheck>,
    compilers: HashMap<&'static str, Arc<dyn ScriptLanguage>>,
    debounce: Option<Duration>,
}

impl Checks {
    fn debounce(&self) -> Duration {
        self.debounce.unwrap_or(DEBOUNCE)
    }

    /// The language's compiler, created once.
    fn compiler(&mut self, language: &'static Language) -> Option<Arc<dyn ScriptLanguage>> {
        let create = language.compiler?;
        Some(
            self.compilers
                .entry(language.id)
                .or_insert_with(create)
                .clone(),
        )
    }
}

impl LanguageServers {
    /// Whether a file of this path has a language with a compiler (so the
    /// editor should hand over its text).
    pub fn has_compiler_for(&self, path: &Path) -> bool {
        LanguageRegistry::current()
            .for_path(path)
            .is_some_and(|language| language.compiler.is_some())
    }

    /// Replace how long a file waits after its last edit (tests shorten it).
    pub fn set_check_debounce(&mut self, debounce: Duration) {
        self.checks.debounce = Some(debounce);
    }

    /// The text of `path` changed (or was opened, or saved): check it after
    /// the debounce. Nothing happens for a file whose language has no
    /// compiler, while no project is open, or while the language's live check
    /// is off (its published set is then cleared).
    pub fn check_source(&mut self, path: &Path, text: &str, cx: &mut Context<Self>) {
        if self.project_root.is_none() {
            return;
        }
        let Some(language) = LanguageRegistry::current().for_path(path) else {
            return;
        };
        let Some(compiler) = self.checks.compiler(language) else {
            return;
        };
        let key = path_key(path);
        let enabled = compiler.live_check_enabled();
        let debounce = self.checks.debounce();
        let file = self.checks.files.entry(key.clone()).or_insert_with(|| FileCheck {
            path: path.to_path_buf(),
            language,
            text: Arc::from(""),
            version: 0,
            due: false,
            running: false,
            done: 0,
            suspended: false,
            _timer: None,
        });
        // The latest text is kept even while the check is off, so turning it on
        // checks what is there now.
        file.text = Arc::from(text);
        file.version += 1;
        file.due = false;
        if !enabled {
            self.suspend_file(&key, path, language, cx);
            return;
        }
        file.suspended = false;
        let version = file.version;
        file._timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(debounce).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(file) = this.checks.files.get_mut(&key) {
                    if file.version == version {
                        file.due = true;
                    }
                }
                this.start_check(&key, cx);
            });
        }));
    }

    /// `path` is closed (or its editor is gone): its compiler diagnostics go,
    /// as the server's do when it closes the document.
    pub fn forget_source(&mut self, path: &Path, cx: &mut Context<Self>) {
        let key = path_key(path);
        if let Some(file) = self.checks.files.remove(&key) {
            self.set_compiler_diagnostics(file.language, &file.path, Vec::new(), cx);
        }
    }

    fn set_compiler_diagnostics(
        &mut self,
        language: &'static Language,
        path: &Path,
        diagnostics: Vec<Diagnostic>,
        cx: &mut Context<Self>,
    ) {
        let source = DiagnosticSource::Compiler(language.id);
        self.diagnostics
            .update(cx, |store, cx| store.set(source, path, diagnostics, cx));
    }

    fn suspend_file(
        &mut self,
        key: &str,
        path: &Path,
        language: &'static Language,
        cx: &mut Context<Self>,
    ) {
        if let Some(file) = self.checks.files.get_mut(key) {
            file.suspended = true;
            file._timer = None;
            file.version += 1;
        }
        self.set_compiler_diagnostics(language, path, Vec::new(), cx);
    }

    /// Run the check of `key` now when it is due and nothing is in flight.
    fn start_check(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(project_root) = self.project_root.clone() else {
            return;
        };
        let Some(file) = self.checks.files.get(key) else {
            return;
        };
        if file.running || !file.due || file.done == file.version || file.suspended {
            return;
        }
        let Some(compiler) = self.checks.compiler(file.language) else {
            return;
        };
        let file = self.checks.files.get_mut(key).expect("looked up above");
        file.running = true;
        let (version, text, path, language) =
            (file.version, file.text.clone(), file.path.clone(), file.language);
        let key = key.to_string();

        let (tx, rx) = flume::bounded::<Vec<CompileDiagnostic>>(1);
        let title = format!(
            "Check {}: {}",
            language.display_name,
            path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
        );
        let job_path = path.clone();
        let job_text = text.clone();
        editor_task_queue::global().submit(
            editor_task_queue::TaskDescription::new(
                title,
                "language-check",
                editor_task_queue::TaskDuration::Short,
            ),
            move |task| {
                task.set_phase("Compiling");
                let natives = NativeRegistry::with_engine_natives();
                let ctx = CheckContext {
                    project_root: &project_root,
                    natives: &natives,
                };
                let found = compiler.check_source(&ctx, &job_path, &job_text);
                let _ = tx.send(found);
                Ok(())
            },
        );
        cx.spawn(async move |this, cx| {
            let found = rx.recv_async().await.ok();
            let _ = this.update(cx, |this, cx| {
                this.finish_check(&key, version, language, &path, &text, found, cx)
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_check(
        &mut self,
        key: &str,
        version: u64,
        language: &'static Language,
        path: &Path,
        text: &str,
        found: Option<Vec<CompileDiagnostic>>,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.checks.files.get_mut(key) else {
            return;
        };
        file.running = false;
        file.done = version;
        let current = file.version == version && !file.suspended;
        if current {
            if let Some(found) = found {
                let diagnostics = compiler_diagnostics(path, text, &found);
                self.set_compiler_diagnostics(language, path, diagnostics, cx);
            }
        } else {
            // Text moved on while this ran: check the latest when it is due.
            file.done = 0;
        }
        self.start_check(key, cx);
    }

    /// Keep the compiler source true (called from the service's tick): drop
    /// what belongs to a plugin that is gone, suspend and resume the files of
    /// a language whose live check was switched off or on.
    pub(super) fn checks_tick(&mut self, cx: &mut Context<Self>) {
        let mut gone: Vec<&'static Language> = Vec::new();
        let mut resume: Vec<String> = Vec::new();
        let mut suspend: Vec<(String, PathBuf, &'static Language)> = Vec::new();
        for (key, file) in &self.checks.files {
            if !pulsar_plugins::origin_enabled(file.language.origin) {
                if !gone.iter().any(|language| language.id == file.language.id) {
                    gone.push(file.language);
                }
                continue;
            }
            let Some(create) = file.language.compiler else {
                continue;
            };
            let enabled = self
                .checks
                .compilers
                .get(file.language.id)
                .cloned()
                .unwrap_or_else(create)
                .live_check_enabled();
            match (enabled, file.suspended) {
                (false, false) => suspend.push((key.clone(), file.path.clone(), file.language)),
                (true, true) => resume.push(key.clone()),
                _ => {}
            }
        }
        for language in gone {
            self.drop_language_checks(language, cx);
        }
        for (key, path, language) in suspend {
            self.suspend_file(&key, &path, language, cx);
        }
        for key in resume {
            if let Some(file) = self.checks.files.get_mut(&key) {
                file.suspended = false;
                file.version += 1;
                file.due = true;
                file.done = 0;
            }
            self.start_check(&key, cx);
        }
    }

    /// A language is gone (its plugin was disabled): its entries and every
    /// file it was checking go.
    pub(super) fn drop_language_checks(&mut self, language: &'static Language, cx: &mut Context<Self>) {
        self.checks.files.retain(|_, file| file.language.id != language.id);
        self.checks.compilers.remove(language.id);
        let source = DiagnosticSource::Compiler(language.id);
        self.diagnostics
            .update(cx, |store, cx| store.clear_source(&source, cx));
    }

    /// A different project opened: nothing of the last one's files is checked
    /// or published any more.
    pub(super) fn drop_all_checks(&mut self, cx: &mut Context<Self>) {
        let mut languages: Vec<&'static Language> = Vec::new();
        for file in self.checks.files.values() {
            if !languages.iter().any(|language| language.id == file.language.id) {
                languages.push(file.language);
            }
        }
        for language in languages {
            self.drop_language_checks(language, cx);
        }
    }

    /// The files being checked now: `(path, version, running)`.
    #[cfg(test)]
    pub(super) fn checked_files(&self) -> Vec<(PathBuf, u64, bool)> {
        self.checks
            .files
            .values()
            .map(|file| (file.path.clone(), file.version, file.running))
            .collect()
    }
}

/// The compiler's problems for `path`, as the diagnostics the editor draws and
/// the drawer lists: one-based line and column from `location`, the range
/// reaching to the end of the word at the position (the compiler reports a
/// point), the source named `Pulsar`.
pub fn compiler_diagnostics(path: &Path, text: &str, found: &[CompileDiagnostic]) -> Vec<Diagnostic> {
    let file_path = path.to_string_lossy().into_owned();
    found
        .iter()
        .map(|problem| {
            let (line, column) = problem
                .location
                .as_deref()
                .and_then(parse_location)
                .unwrap_or((1, 1));
            let end_column = word_end(text, line, column);
            Diagnostic {
                file_path: file_path.clone(),
                line,
                column,
                end_line: Some(line),
                end_column: Some(end_column),
                severity: match problem.severity {
                    CompileSeverity::Error => DiagnosticSeverity::Error,
                    CompileSeverity::Warning => DiagnosticSeverity::Warning,
                },
                message: problem.message.clone(),
                code: None,
                source: Some(COMPILER_SOURCE.to_string()),
                code_actions: Vec::new(),
                raw_lsp_diagnostic: None,
            }
        })
        .collect()
}

/// `"12:5"` as one-based `(line, column)`.
fn parse_location(location: &str) -> Option<(usize, usize)> {
    let (line, column) = location.split_once(':')?;
    let (line, column) = (line.trim().parse().ok()?, column.trim().parse().ok()?);
    (line >= 1 && column >= 1).then_some((line, column))
}

/// The one-based column just past the identifier-like run that starts at
/// `column` of `line` (at least one character wide).
fn word_end(text: &str, line: usize, column: usize) -> usize {
    let Some(content) = text.lines().nth(line - 1) else {
        return column + 1;
    };
    let tail: Vec<char> = content.chars().skip(column - 1).collect();
    let run = tail
        .iter()
        .take_while(|c| c.is_alphanumeric() || **c == '_' || **c == '$')
        .count();
    column + run.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(location: Option<&str>, message: &str) -> CompileDiagnostic {
        CompileDiagnostic {
            severity: CompileSeverity::Error,
            class: Some("Door".into()),
            file: None,
            location: location.map(str::to_owned),
            message: message.into(),
        }
    }

    #[test]
    fn a_compiler_problem_becomes_a_pulsar_diagnostic_with_a_range() {
        let text = "export default class Door {\n    f(): int { return missing_call(1); }\n}\n";
        let found = [problem(Some("2:23"), "unknown function")];
        let diagnostics = compiler_diagnostics(Path::new("/p/src/classes/Door/class.ts"), text, &found);
        assert_eq!(diagnostics.len(), 1);
        let d = &diagnostics[0];
        assert_eq!((d.line, d.column), (2, 23));
        assert_eq!((d.end_line, d.end_column), (Some(2), Some(23 + "missing_call".len())));
        assert_eq!(d.source.as_deref(), Some("Pulsar"));
        assert_eq!(d.severity, DiagnosticSeverity::Error);
        assert!(d.raw_lsp_diagnostic.is_none());
    }

    #[test]
    fn a_problem_without_a_position_sits_at_the_top_and_a_point_is_one_wide() {
        let d = &compiler_diagnostics(Path::new("a.ts"), "x", &[problem(None, "no class")])[0];
        assert_eq!((d.line, d.column, d.end_column), (1, 1, Some(2)));
        let d = &compiler_diagnostics(Path::new("a.ts"), "a + ;", &[problem(Some("1:3"), "x")])[0];
        assert_eq!(d.end_column, Some(4), "an operator is one character");
        let d = &compiler_diagnostics(Path::new("a.ts"), "", &[problem(Some("9:9"), "x")])[0];
        assert_eq!(d.end_column, Some(10), "a position past the text is still one wide");
    }

    #[test]
    fn locations_parse_one_based_and_reject_nonsense() {
        assert_eq!(parse_location("3:14"), Some((3, 14)));
        assert_eq!(parse_location("0:1"), None);
        assert_eq!(parse_location("node n1"), None);
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// What the fake compiler saw and whether its live check is on.
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static LAST_TEXT: Mutex<String> = Mutex::new(String::new());
    static ENABLED: AtomicBool = AtomicBool::new(true);

    struct FakeCompiler;

    impl ScriptLanguage for FakeCompiler {
        fn live_check_enabled(&self) -> bool {
            ENABLED.load(Ordering::SeqCst)
        }

        fn check_source(&self, ctx: &CheckContext, path: &Path, text: &str) -> Vec<CompileDiagnostic> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            *LAST_TEXT.lock().unwrap() = text.to_string();
            assert!(ctx.project_root.is_absolute() || ctx.project_root.starts_with("/"));
            vec![CompileDiagnostic {
                severity: CompileSeverity::Error,
                class: None,
                file: Some(path.to_path_buf()),
                location: Some("1:1".into()),
                message: format!("saw: {text}"),
            }]
        }
    }

    const LANGUAGE: Language = Language {
        origin: pulsar_plugins::engine_origin!(),
        id: "check-lang",
        display_name: "Check language",
        file_extensions: &["chk"],
        language_ids: &[],
        highlight_id: "",
        class_markers: &[],
        config_files: &[],
        compiler: Some(|| Arc::new(FakeCompiler)),
        server: None,
    };

    pulsar_plugins::inventory::submit! { LANGUAGE }

    /// The service only checks files while a project is open.
    fn service(cx: &mut TestAppContext) -> (gpui::Entity<LanguageServers>, gpui::Entity<crate::DiagnosticsStore>, PathBuf) {
        let project = std::env::temp_dir().join(format!("pulsar_lsp_check_{}", std::process::id()));
        std::fs::create_dir_all(&project).unwrap();
        let service = cx.new(LanguageServers::new);
        service.update(cx, |service, cx| {
            service.set_project(project.clone(), cx);
            service.set_check_debounce(Duration::from_millis(50));
        });
        let store = service.read_with(cx, |service, _| service.diagnostics());
        (service, store, project.join("a.chk"))
    }

    fn wait_for(cx: &mut TestAppContext, what: &str, mut done: impl FnMut(&mut TestAppContext) -> bool) {
        cx.executor().allow_parking();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The tests share the fake compiler's counters: one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[gpui::test]
    fn a_burst_of_edits_is_one_check_of_the_latest_text(cx: &mut TestAppContext) {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        ENABLED.store(true, Ordering::SeqCst);
        CALLS.store(0, Ordering::SeqCst);
        let (service, store, file) = service(cx);
        assert!(service.read_with(cx, |s, _| s.has_compiler_for(&file)));
        assert!(!service.read_with(cx, |s, _| s.has_compiler_for(&file.with_extension("txt"))));

        for text in ["one", "two", "three", "four"] {
            service.update(cx, |s, cx| s.check_source(&file, text, cx));
        }
        // Inside the debounce nothing has run.
        cx.run_until_parked();
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);
        cx.executor().advance_clock(Duration::from_secs(1));
        wait_for(cx, "the check", |cx| !store.read_with(cx, |s, _| s.for_file(&file)).is_empty());

        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "one check for the whole burst");
        assert_eq!(&*LAST_TEXT.lock().unwrap(), "four", "the latest text wins");
        let entries = store.read_with(cx, |s, _| s.for_file(&file));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, DiagnosticSource::Compiler("check-lang"));
        assert_eq!(entries[0].1.message, "saw: four");
        assert_eq!(entries[0].1.source.as_deref(), Some("Pulsar"));
    }

    #[gpui::test]
    fn an_edit_during_a_check_is_checked_next_and_never_overwritten_by_the_old_result(
        cx: &mut TestAppContext,
    ) {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        ENABLED.store(true, Ordering::SeqCst);
        CALLS.store(0, Ordering::SeqCst);
        let (service, store, file) = service(cx);
        service.update(cx, |s, cx| s.check_source(&file, "first", cx));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        // Whether or not the first has finished, a newer text supersedes it.
        service.update(cx, |s, cx| s.check_source(&file, "second", cx));
        cx.executor().advance_clock(Duration::from_secs(1));
        wait_for(cx, "the latest result", |cx| {
            store
                .read_with(cx, |s, _| s.for_file(&file))
                .first()
                .is_some_and(|(_, d)| d.message == "saw: second")
        });
        assert!(service.read_with(cx, |s, _| s.checked_files().iter().all(|(_, _, running)| !running)));
    }

    #[gpui::test]
    fn switching_the_live_check_off_clears_the_source_and_on_checks_the_open_files_again(
        cx: &mut TestAppContext,
    ) {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        ENABLED.store(true, Ordering::SeqCst);
        let (service, store, file) = service(cx);
        service.update(cx, |s, cx| s.check_source(&file, "text", cx));
        cx.executor().advance_clock(Duration::from_secs(1));
        wait_for(cx, "the check", |cx| !store.read_with(cx, |s, _| s.for_file(&file)).is_empty());

        // The language's setting turns off: the next tick clears the source.
        ENABLED.store(false, Ordering::SeqCst);
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        assert!(store.read_with(cx, |s, _| s.for_file(&file)).is_empty());
        // And an edit while it is off checks nothing.
        let calls = CALLS.load(Ordering::SeqCst);
        service.update(cx, |s, cx| s.check_source(&file, "more", cx));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(CALLS.load(Ordering::SeqCst), calls);
        assert!(store.read_with(cx, |s, _| s.for_file(&file)).is_empty());

        // Back on: the open file is checked again without another edit.
        ENABLED.store(true, Ordering::SeqCst);
        cx.executor().advance_clock(Duration::from_secs(5));
        wait_for(cx, "the resumed check", |cx| !store.read_with(cx, |s, _| s.for_file(&file)).is_empty());
        let entries = store.read_with(cx, |s, _| s.for_file(&file));
        assert_eq!(entries[0].1.message, "saw: more", "the latest text is the one checked");
    }
}
