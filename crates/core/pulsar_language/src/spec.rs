//! What the host needs to run and talk to one language server.
//!
//! A [`LanguageServerSpec`] is plain data and function pointers: it names the
//! server, finds (or obtains) its executable, says where its roots are and how
//! it is configured, and carries the hooks for what only that server knows.
//! It is the `server` of a [`Language`](crate::Language): which files the
//! server handles, the `languageId`s and the config files belong to the
//! language, not to the spec. The generic client (`pulsar_lsp::LanguageServer`)
//! owns the process and the protocol; a spec never touches either.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::settings::ServerSettings;

/// How to launch a located server.
#[derive(Clone, Debug, Default)]
pub struct ServerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

impl ServerCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }
}

/// What [`LanguageServerSpec::locate`] is given. `locate` runs on the
/// server's own thread: it may block, probe, install or download, and reports
/// what it is doing through [`LocateContext::report`].
pub struct LocateContext<'a> {
    pub project_root: &'a Path,
    /// The person's `path` setting for this server, if any: it is used or the
    /// server is reported missing, never silently replaced by another binary.
    pub configured_path: Option<PathBuf>,
    pub(crate) progress: &'a dyn Fn(&str),
}

impl<'a> LocateContext<'a> {
    pub fn new(
        project_root: &'a Path,
        configured_path: Option<PathBuf>,
        progress: &'a dyn Fn(&str),
    ) -> Self {
        Self {
            project_root,
            configured_path,
            progress,
        }
    }

    /// Tell the footer what location is doing ("Installing rust-analyzer...").
    pub fn report(&self, text: &str) {
        (self.progress)(text);
    }
}

/// Why a server could not be found or obtained: shown to the person as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocateError {
    /// What the person reads. It says what is missing and what to do about it.
    pub message: String,
    /// The server (or what it runs on) is not there, as opposed to being there
    /// and failing. The footer then shows a "missing" status with the spec's
    /// install action, when it has one, rather than a failure.
    pub missing: bool,
}

impl LocateError {
    /// A failure to find or run the server.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            missing: false,
        }
    }

    /// The server, or the runtime it needs, is not installed.
    pub fn missing(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            missing: true,
        }
    }
}

impl std::fmt::Display for LocateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<anyhow::Error> for LocateError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(format!("{error:#}"))
    }
}

/// What an install is told about its surroundings: where to say what it is
/// doing. An install runs as an editor task on a pool worker.
pub struct InstallContext<'a> {
    pub(crate) progress: &'a dyn Fn(&str),
}

impl<'a> InstallContext<'a> {
    pub fn new(progress: &'a dyn Fn(&str)) -> Self {
        Self { progress }
    }

    /// Tell the footer and the task's log what the install is doing.
    pub fn report(&self, text: &str) {
        (self.progress)(text);
    }
}

/// A managed install of a server: what the person is offered when the server
/// is not found, and the code that does it once they have said yes. It is
/// never run silently: the host runs it only on the person's explicit action
/// (the footer's Install item), as an editor task with its own log.
pub struct ServerInstaller {
    /// The action's label ("Install typescript-language-server").
    pub label: &'static str,
    /// What the install will do, shown before the person approves it: what is
    /// downloaded, from where, and where it goes.
    pub describe: fn() -> String,
    /// Do it. The download is verified against the digest pinned in the plugin
    /// before anything is unpacked; a mismatch is an error and leaves nothing
    /// behind.
    pub install: fn(&InstallContext) -> Result<(), LocateError>,
}

/// Where the servers of one spec run.
#[derive(Clone, Copy, Debug)]
pub enum RootStrategy {
    /// One server for the whole project.
    ProjectRoot,
    /// The nearest directory at or above the file (up to the project root)
    /// holding one of the marker files; the project root when none does.
    NearestMarker(&'static [&'static str]),
    /// The outermost such directory: a workspace's root, not each member's.
    OutermostMarker(&'static [&'static str]),
    /// The spec decides (`file`, `project_root`); `None` means no server.
    Custom(fn(&Path, &Path) -> Option<PathBuf>),
}

impl RootStrategy {
    /// The root of the server that handles `file`.
    pub fn root_for(&self, file: &Path, project_root: &Path) -> Option<PathBuf> {
        let has_marker = |dir: &Path, markers: &[&str]| {
            markers
                .iter()
                .any(|marker| engine_fs::virtual_fs::is_file(&dir.join(marker)))
        };
        let ancestors = || {
            file.parent()
                .into_iter()
                .flat_map(Path::ancestors)
                .take_while(|dir| dir.starts_with(project_root))
        };
        match self {
            Self::ProjectRoot => Some(project_root.to_path_buf()),
            Self::NearestMarker(markers) => Some(
                ancestors()
                    .find(|dir| has_marker(dir, markers))
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| project_root.to_path_buf()),
            ),
            Self::OutermostMarker(markers) => Some(
                ancestors()
                    .filter(|dir| has_marker(dir, markers))
                    .last()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| project_root.to_path_buf()),
            ),
            Self::Custom(root) => root(file, project_root),
        }
    }
}

/// When a server is ready to serve requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessRule {
    /// As soon as it has answered `initialize`.
    Immediate,
    /// After it has answered `initialize`, began work-done progress and every
    /// progress token has ended.
    AllProgressEnded,
    /// When the spec's `notification` hook returns [`ServerSignal::Ready`].
    Signal,
}

/// What a server-specific notification means to the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerSignal {
    /// The server finished its initial work.
    Ready,
}

/// A `$/progress` message, as the spec's `progress` hook sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressEvent<'a> {
    pub token: &'a str,
    pub kind: ProgressKind,
    /// The title the token began with.
    pub title: &'a str,
    pub message: Option<&'a str>,
    /// 0 to 100.
    pub percentage: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressKind {
    Begin,
    Report,
    End,
}

/// What a progress message shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressText {
    pub text: String,
    /// 0.0 to 1.0, when the server says.
    pub fraction: Option<f32>,
}

/// A text edit of a quick fix: one-based lines and columns, as the Problems
/// drawer shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickFixEdit {
    pub file_path: String,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub new_text: String,
}

/// A fix a server offers for a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickFix {
    pub title: String,
    pub edits: Vec<QuickFixEdit>,
}

/// Hooks for what only one server knows. Every hook has a neutral default.
#[derive(Clone, Copy)]
pub struct ServerHooks {
    /// Text for a begin or report; `None` shows nothing for it. The default
    /// shows `title: message`.
    pub progress: fn(&ProgressEvent) -> Option<ProgressText>,
    /// A server-specific notification (`rust-analyzer/serverStatus`).
    pub notification: fn(&str, &Value) -> Option<ServerSignal>,
    /// The quick fixes a server ships inside one raw `publishDiagnostics`
    /// diagnostic (`data.fixes`, related information) and the path of its
    /// file; the host attaches them to the diagnostic it shows.
    pub quick_fixes: fn(&Value, &str) -> Vec<QuickFix>,
}

impl ServerHooks {
    pub const DEFAULT: Self = Self {
        progress: default_progress,
        notification: |_, _| None,
        quick_fixes: |_, _| Vec::new(),
    };
}

impl Default for ServerHooks {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn default_progress(event: &ProgressEvent) -> Option<ProgressText> {
    let text = match (event.title.is_empty(), event.message) {
        (false, Some(message)) if !message.is_empty() => format!("{}: {message}", event.title),
        (false, _) => event.title.to_string(),
        (true, Some(message)) => message.to_string(),
        (true, None) => return None,
    };
    Some(ProgressText {
        text,
        fraction: event.percentage.map(|p| p.min(100) as f32 / 100.0),
    })
}

/// What a [`ServerCommandSpec`] does with the server's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandResult {
    /// Open the `Location | Location[] | LocationLink[]` answered.
    OpenLocation,
    /// Open the text the answer carries (`name`, `expansion`) in a scratch
    /// file named by `scratch_extension`.
    ShowText,
    /// Nothing to show.
    None,
}

/// What a [`ServerCommandSpec`] needs from the editor to build its request.
#[derive(Clone, Debug)]
pub struct CommandContext {
    pub uri: String,
    /// Zero-based line and UTF-16 column of the caret.
    pub line: usize,
    pub column: usize,
}

/// A request only this server understands, offered by the editor's menu
/// ("Expand macro", "Open Cargo.toml").
#[derive(Clone, Copy)]
pub struct ServerCommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub method: &'static str,
    /// Whether it needs a caret position (otherwise only the document).
    pub needs_position: bool,
    /// Whether it asks about the document (`false`: the whole workspace).
    pub needs_document: bool,
    pub params: fn(&CommandContext) -> Value,
    pub result: CommandResult,
    /// For [`CommandResult::ShowText`]: the scratch file's extension.
    pub scratch_extension: &'static str,
    /// Pulls `(title, body)` out of the answer, for [`CommandResult::ShowText`].
    pub text: fn(&Value) -> Option<(String, String)>,
}

/// What a spec's callbacks are told about the server they configure.
#[derive(Clone, Debug)]
pub struct ServerContext {
    /// The server's root (what it was started in).
    pub root: PathBuf,
    pub project_root: PathBuf,
    pub settings: ServerSettings,
}

/// Everything about one language server.
pub struct LanguageServerSpec {
    /// Stable id for status, logs, settings and diagnostics ("rust-analyzer").
    pub id: &'static str,
    pub display_name: &'static str,
    /// Find the executable without installing anything (the loading screen's
    /// check, and the first half of `locate`). `None`: not found.
    pub find: fn(&LocateContext) -> Option<ServerCommand>,
    /// Find, or obtain, the executable. Called on the server's thread; it may
    /// install or download, reporting through the context.
    pub locate: fn(&LocateContext) -> Result<ServerCommand, LocateError>,
    pub roots: RootStrategy,
    /// The `initializationOptions` value (also the base of the answer to
    /// `workspace/configuration` when `configuration` is the default).
    pub initialization_options: fn(&ServerContext) -> Value,
    /// The answer to `workspace/configuration` for one section.
    pub configuration: fn(&ServerContext, &str) -> Value,
    /// Extra client capabilities merged into the host's base set.
    pub client_capabilities: fn() -> Value,
    pub hooks: ServerHooks,
    pub readiness: ReadinessRule,
    /// A server that has not become ready this long after `initialize` is
    /// declared ready (a warning in its log), so no status is stuck for good.
    pub max_indexing: Duration,
    /// Starts when the project opens (otherwise on its first matching file).
    pub eager: bool,
    pub commands: &'static [ServerCommandSpec],
    /// Written into the workspace before the server starts.
    pub prepare_workspace: Option<fn(&ServerContext) -> Result<(), String>>,
    /// A managed install, offered (never run unasked) when `locate` reports
    /// the server missing.
    pub installer: Option<&'static ServerInstaller>,
}

impl LanguageServerSpec {
    /// The `initializationOptions` with the person's overlay merged on top.
    pub fn effective_initialization_options(&self, ctx: &ServerContext) -> Value {
        let mut options = (self.initialization_options)(ctx);
        if let Some(overlay) = &ctx.settings.initialization_options {
            merge_json(&mut options, overlay);
        }
        options
    }
}

/// Merge `overlay` into `base`: objects merge key by key, anything else
/// replaces.
pub fn merge_json(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(key) {
                    Some(existing) => merge_json(existing, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

/// The default `workspace/configuration` answer: the section looked up in the
/// effective initialization options (`a.b` walks `a` then `b`), else null.
pub fn section_of(options: &Value, section: &str) -> Value {
    if section.is_empty() {
        return options.clone();
    }
    let mut current = options;
    for part in section.split('.') {
        match current.get(part) {
            Some(next) => current = next,
            None => return Value::Null,
        }
    }
    current.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn overlay_merges_objects_and_replaces_scalars() {
        let mut base = json!({ "check": { "command": "clippy", "extra": 1 }, "a": 1 });
        merge_json(&mut base, &json!({ "check": { "command": "check" }, "b": 2 }));
        assert_eq!(
            base,
            json!({ "check": { "command": "check", "extra": 1 }, "a": 1, "b": 2 })
        );
    }

    #[test]
    fn a_configuration_section_is_found_by_dotted_path() {
        let options = json!({ "rust-analyzer": { "check": { "command": "clippy" } } });
        assert_eq!(
            section_of(&options, "rust-analyzer.check.command"),
            json!("clippy")
        );
        assert_eq!(section_of(&options, "missing"), Value::Null);
        assert_eq!(section_of(&options, ""), options);
    }

    #[test]
    fn default_progress_joins_title_and_message() {
        let event = ProgressEvent {
            token: "t",
            kind: ProgressKind::Report,
            title: "Indexing",
            message: Some("3/10"),
            percentage: Some(30),
        };
        let text = (ServerHooks::DEFAULT.progress)(&event).unwrap();
        assert_eq!(text.text, "Indexing: 3/10");
        assert_eq!(text.fraction, Some(0.3));
    }

    #[test]
    fn the_nearest_marker_wins_and_the_outermost_marker_wins() {
        let root = std::env::temp_dir().join(format!("pulsar_lsp_roots_{}", std::process::id()));
        let member = root.join("crates").join("member").join("src");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]").unwrap();
        std::fs::write(root.join("crates").join("member").join("Cargo.toml"), "").unwrap();
        let file = member.join("lib.rs");
        let markers: &'static [&'static str] = &["Cargo.toml"];
        assert_eq!(
            RootStrategy::NearestMarker(markers).root_for(&file, &root),
            Some(root.join("crates").join("member"))
        );
        assert_eq!(
            RootStrategy::OutermostMarker(markers).root_for(&file, &root),
            Some(root.clone())
        );
        assert_eq!(
            RootStrategy::ProjectRoot.root_for(&file, &root),
            Some(root.clone())
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
