//! One language server process: the generic client.
//!
//! A [`LanguageServer`] is an entity for one `(server, root)`. Its process
//! runs on a registered service thread (`editor_task_queue::spawn_service`,
//! shown in the Tasks window with its own log): the thread locates and
//! spawns the executable, performs the handshake, then reads the server's
//! output until it ends, answering the server's own requests as they come
//! ([`crate::protocol`]). What the UI needs is sent to the entity over a
//! channel and applied there, so the entity never blocks and nothing polls
//! per frame.
//!
//! The entity owns the state a person sees ([`ServerStatus`], through
//! [`Session`]) and the documents the server has open ([`Documents`]); it
//! emits [`ServerEvent`]s and writes the server's diagnostics into the
//! shared [`DiagnosticsStore`].

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures::{select_biased, FutureExt as _};
use gpui::{Context, Entity, EventEmitter, Task};
use serde_json::{json, Value};

use crate::capabilities;
use crate::code_actions;
use crate::diagnostics::{parse_publish, DiagnosticSource, DiagnosticsStore, ServerId};
use crate::documents::{Documents, Notifications};
use crate::edit::{apply_workspace_edit_in_app, WorkspaceEdit};
use crate::path_utils::{path_to_uri, uri_to_path};
use crate::protocol::{self, Connection, Handler, RequestOutcome, METHOD_NOT_FOUND};
use crate::session::{ServerEvent, ServerStatus, Session, StopReason};
use pulsar_language::settings::{ServerSettings, TraceLevel};
use pulsar_language::Language;
use pulsar_language::spec::{
    section_of, InstallContext, LanguageServerSpec, LocateContext, LocateError, ProgressKind,
    ServerCommand, ServerCommandSpec, ServerContext, ServerInstaller, ServerSignal,
};

/// How long a server may spend locating, spawning and answering `initialize`
/// before it is reported as failed (a hard cap, not a readiness heuristic).
const START_CAP: Duration = Duration::from_secs(300);

/// How often the entity checks the readiness cap and the start cap.
const TICK: Duration = Duration::from_secs(1);

/// What the server's thread talks over.
enum Transport {
    /// Locate the executable, spawn it, talk over its stdio.
    Process,
    /// Talk over streams that are already connected.
    Attached(Box<dyn std::io::Read + Send>, Box<dyn std::io::Write + Send>),
}

/// What the server's thread tells the entity.
enum Msg {
    /// Something the footer can show while starting ("Installing ...").
    Detail(String),
    Spawned(Arc<Connection>),
    Initialized { capabilities: Value },
    Failed(String),
    /// The server or its runtime is not installed.
    Missing(String),
    Progress {
        token: String,
        kind: ProgressKind,
        title: Option<String>,
        message: Option<String>,
        percentage: Option<u32>,
    },
    Signal(ServerSignal),
    Diagnostics { path: PathBuf, diagnostics: Vec<ui::diagnostics::Diagnostic> },
    ApplyEdit { id: Value, params: Value },
    Exited(String),
}

pub struct LanguageServer {
    language: &'static Language,
    spec: &'static LanguageServerSpec,
    id: ServerId,
    project_root: PathBuf,
    settings: ServerSettings,
    session: Session,
    documents: Mutex<Documents>,
    connection: Option<Arc<Connection>>,
    child: Arc<Mutex<Option<Child>>>,
    service: Option<editor_task_queue::ServiceHandle<()>>,
    /// The person-approved install in flight (see [`LanguageServer::install`]).
    installing: bool,
    _install_pump: Option<Task<()>>,
    diagnostics: Entity<DiagnosticsStore>,
    capabilities: Value,
    triggers: Vec<char>,
    /// Identifies the current process: messages of an older one are dropped.
    generation: u64,
    started_at: Option<Instant>,
    /// What starting is doing now ("Installing the server...").
    detail: String,
    last_activity: Instant,
    _pump: Option<Task<()>>,
}

impl EventEmitter<ServerEvent> for LanguageServer {}

impl LanguageServer {
    /// A server of `language`, which has `spec` as its server.
    pub fn new(
        language: &'static Language,
        spec: &'static LanguageServerSpec,
        root: PathBuf,
        project_root: PathBuf,
        settings: ServerSettings,
        diagnostics: Entity<DiagnosticsStore>,
    ) -> Self {
        Self {
            language,
            spec,
            id: ServerId { server: spec.id, root },
            project_root,
            settings,
            session: Session::new(spec),
            documents: Mutex::new(Documents::default()),
            connection: None,
            child: Arc::new(Mutex::new(None)),
            service: None,
            installing: false,
            _install_pump: None,
            diagnostics,
            capabilities: Value::Null,
            triggers: Vec::new(),
            generation: 0,
            started_at: None,
            detail: String::new(),
            last_activity: Instant::now(),
            _pump: None,
        }
    }

    pub fn id(&self) -> &ServerId {
        &self.id
    }

    /// The language this server is of.
    pub fn language(&self) -> &'static Language {
        self.language
    }

    pub fn spec(&self) -> &'static LanguageServerSpec {
        self.spec
    }

    pub fn root(&self) -> &Path {
        &self.id.root
    }

    pub fn settings(&self) -> &ServerSettings {
        &self.settings
    }

    pub fn status(&self) -> &ServerStatus {
        self.session.status()
    }

    pub fn is_running(&self) -> bool {
        self.session.status().is_running()
    }

    /// Whether the server has answered `initialize`: it can take documents
    /// and requests.
    pub fn is_initialized(&self) -> bool {
        self.session.status().is_initialized()
    }

    /// What starting is doing now (locating, installing); empty otherwise.
    pub fn detail(&self) -> &str {
        match self.session.status() {
            ServerStatus::Starting => &self.detail,
            _ => "",
        }
    }

    pub fn diagnostics(&self) -> &Entity<DiagnosticsStore> {
        &self.diagnostics
    }

    pub fn commands(&self) -> &'static [ServerCommandSpec] {
        self.spec.commands
    }

    /// The characters the server asked completion on (empty until it has
    /// answered `initialize`).
    pub fn completion_trigger_characters(&self) -> Vec<char> {
        self.triggers.clone()
    }

    /// Whether the server takes incremental changes (otherwise full text).
    pub fn supports_incremental_sync(&self) -> bool {
        let sync = self.capabilities.get("textDocumentSync");
        let kind = sync
            .and_then(|sync| sync.as_u64().or_else(|| sync.get("change").and_then(Value::as_u64)));
        kind == Some(2)
    }

    pub fn open_document_count(&self) -> usize {
        self.documents.lock().map(|docs| docs.len()).unwrap_or(0)
    }

    pub fn last_activity(&self) -> Instant {
        self.last_activity
    }

    fn context(&self) -> ServerContext {
        ServerContext {
            root: self.id.root.clone(),
            project_root: self.project_root.clone(),
            settings: self.settings.clone(),
        }
    }

    fn emit_all(&mut self, events: Vec<ServerEvent>, cx: &mut Context<Self>) {
        let stopped = events.iter().any(|event| {
            matches!(event, ServerEvent::Error(_) | ServerEvent::Stopped(_))
        });
        for event in events {
            cx.emit(event);
        }
        if stopped {
            // The process is gone or going: it knows no documents and its
            // diagnostics are no longer true.
            self.on_process_gone(cx);
        }
        cx.notify();
    }

    fn on_process_gone(&mut self, cx: &mut Context<Self>) {
        if let Ok(mut documents) = self.documents.lock() {
            documents.forget_all();
        }
        self.triggers.clear();
        self.capabilities = Value::Null;
        let id = self.id.clone();
        self.diagnostics
            .update(cx, |store, cx| store.clear_server(&id, cx));
    }

    // ---- lifecycle --------------------------------------------------------

    /// Start the process (a no-op while one is running).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.start_with(Transport::Process, cx);
    }

    /// Run the server over an existing pair of streams instead of spawning a
    /// process: the spec's workspace preparation and the handshake still run,
    /// but nothing is located or launched. For servers that are already
    /// connected, and for tests that script a server on a pipe.
    pub fn attach(
        &mut self,
        reader: impl std::io::Read + Send + 'static,
        writer: impl std::io::Write + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.start_with(Transport::Attached(Box::new(reader), Box::new(writer)), cx);
    }

    fn start_with(&mut self, transport: Transport, cx: &mut Context<Self>) {
        if self.is_running() || self.installing {
            return;
        }
        self.kill_process();
        self.generation += 1;
        let generation = self.generation;
        self.started_at = Some(Instant::now());
        self.detail.clear();
        let events = self.session.starting();
        for event in events {
            cx.emit(event);
        }

        let (tx, rx) = flume::unbounded::<Msg>();
        let context = Arc::new(self.context());
        let spec = self.spec;
        let child_slot = self.child.clone();
        let title = format!("{} ({})", spec.display_name, self.id.root.display());
        let description = editor_task_queue::ServiceDescription::new(
            format!("Language server: {title}"),
            "language-server",
            format!("lsp: {}", spec.id),
            editor_task_queue::ThreadRole::Io,
        )
        .owner("pulsar_lsp")
        .heartbeat(Duration::from_secs(30));
        let thread_tx = tx.clone();
        let spawned = editor_task_queue::spawn_service(description, move |task| {
            run_server(spec, context, child_slot, thread_tx, task, transport);
        });
        match spawned {
            Ok(handle) => self.service = Some(handle),
            Err(error) => {
                let events = self.session.failed(format!("could not start a thread: {error}"));
                self.emit_all(events, cx);
                return;
            }
        }
        drop(tx);

        // The pump: apply what the thread sends, and tick the caps.
        self._pump = Some(cx.spawn(async move |this, cx| {
            loop {
                let next = select_biased! {
                    message = rx.recv_async().fuse() => Some(message),
                    _ = cx.background_executor().timer(TICK).fuse() => None,
                };
                let alive = match next {
                    Some(Ok(message)) => this
                        .update(cx, |this, cx| this.on_message(generation, message, cx))
                        .is_ok(),
                    Some(Err(_)) => break,
                    None => this.update(cx, |this, cx| this.on_tick(generation, cx)).is_ok(),
                };
                if !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn on_tick(&mut self, generation: u64, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        let mut events = self.session.tick(Instant::now());
        if matches!(self.session.status(), ServerStatus::Starting) {
            if let Some(started) = self.started_at {
                if started.elapsed() > START_CAP {
                    self.kill_process();
                    events.extend(self.session.failed(format!(
                        "{} did not start within {} seconds",
                        self.spec.display_name,
                        START_CAP.as_secs()
                    )));
                }
            }
        }
        if !events.is_empty() {
            self.emit_all(events, cx);
        }
    }

    fn on_message(&mut self, generation: u64, message: Msg, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        let now = Instant::now();
        match message {
            Msg::Detail(text) => {
                self.detail = text.clone();
                let events = self.session.starting_detail(&text);
                self.emit_all(events, cx);
            }
            Msg::Spawned(connection) => {
                connection.set_trace(self.settings.trace);
                self.connection = Some(connection);
            }
            Msg::Initialized { capabilities } => {
                self.triggers = capabilities
                    .pointer("/completionProvider/triggerCharacters")
                    .and_then(Value::as_array)
                    .map(|chars| {
                        chars
                            .iter()
                            .filter_map(|c| c.as_str()?.chars().next())
                            .collect()
                    })
                    .unwrap_or_default();
                self.capabilities = capabilities;
                let events = self.session.initialized(now);
                // The session is initialized now: announce what editors
                // opened while it started.
                let flushed = self.documents.lock().map(|mut docs| docs.flush()).unwrap_or_default();
                self.send_all(flushed);
                if self.settings.trace != TraceLevel::Off {
                    let _ = self.notify(
                        "$/setTrace",
                        json!({ "value": self.settings.trace.as_str() }),
                    );
                }
                self.emit_all(events, cx);
            }
            Msg::Failed(error) => {
                self.kill_process();
                let events = self.session.failed(error);
                self.emit_all(events, cx);
            }
            Msg::Missing(message) => {
                self.kill_process();
                let events = self.session.missing(message);
                self.emit_all(events, cx);
            }
            Msg::Progress { token, kind, title, message, percentage } => {
                let events = self.session.progress(
                    &token,
                    kind,
                    title.as_deref(),
                    message.as_deref(),
                    percentage,
                    now,
                );
                self.emit_all(events, cx);
            }
            Msg::Signal(signal) => {
                let events = self.session.signal(signal);
                self.emit_all(events, cx);
            }
            Msg::Diagnostics { path, diagnostics } => {
                let source = DiagnosticSource::Server(self.id.clone());
                self.diagnostics
                    .update(cx, |store, cx| store.set(source, &path, diagnostics, cx));
            }
            Msg::ApplyEdit { id, params } => {
                let outcome = match params
                    .get("edit")
                    .ok_or_else(|| "workspace/applyEdit without an edit".to_string())
                    .and_then(WorkspaceEdit::from_value)
                    .and_then(|edit| apply_workspace_edit_in_app(&edit, cx))
                {
                    Ok(_) => json!({ "applied": true }),
                    Err(reason) => {
                        tracing::warn!("[{}] workspace/applyEdit refused: {reason}", self.id.server);
                        json!({ "applied": false, "failureReason": reason })
                    }
                };
                if let Some(connection) = &self.connection {
                    let _ = connection.respond(&id, Ok(outcome));
                }
            }
            Msg::Exited(how) => {
                self.child.lock().ok().and_then(|mut slot| slot.take());
                self.connection = None;
                if self.session.status().is_running() {
                    let events = self.session.exited(&how);
                    self.emit_all(events, cx);
                }
            }
        }
    }

    fn kill_process(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
        if let Some(service) = self.service.take() {
            service.request_stop();
        }
        if let Some(mut child) = self.child.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Stop the process. Its documents and diagnostics go with it.
    pub fn stop(&mut self, reason: StopReason, cx: &mut Context<Self>) {
        self.generation += 1;
        self.kill_process();
        self._pump = None;
        let events = self.session.stopped(reason);
        self.emit_all(events, cx);
    }

    /// Stop and start again. The pause is not a sleep: the old process is
    /// gone before this returns, and the new one starts on its own thread.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        self.stop(StopReason::Requested, cx);
        self.start(cx);
    }

    /// Whether this server's spec offers a managed install.
    pub fn installer(&self) -> Option<&'static ServerInstaller> {
        self.spec.installer
    }

    /// Whether an install is running now.
    pub fn is_installing(&self) -> bool {
        self.installing
    }

    /// Run the spec's managed install, then start the server. Only ever called
    /// for a person's explicit action (the footer's Install item): the host
    /// never installs on its own. The install is an editor task (Tasks window,
    /// its own log); the footer shows what it reports. A failure is the
    /// server's error status, with the install's message.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Some(installer) = self.spec.installer else {
            return;
        };
        if self.installing || self.is_running() {
            return;
        }
        self.installing = true;
        self.generation += 1;
        let events = self.session.starting();
        for event in events {
            cx.emit(event);
        }
        let name = self.spec.display_name;
        let (tx, rx) = flume::unbounded::<Result<(), LocateError>>();
        let (detail_tx, detail_rx) = flume::unbounded::<String>();
        let description = editor_task_queue::TaskDescription::new(
            format!("Install {name}"),
            "language-server",
            editor_task_queue::TaskDuration::Long,
        );
        editor_task_queue::global().submit(description, move |task| {
            task.set_phase("Installing");
            let progress = |text: &str| {
                tracing::info!("{text}");
                let _ = detail_tx.send(text.to_string());
            };
            let result = (installer.install)(&InstallContext::new(&progress));
            let outcome = match &result {
                Ok(()) => Ok(()),
                Err(error) => Err(error.message.clone()),
            };
            let _ = tx.send(result);
            outcome
        });
        self._install_pump = Some(cx.spawn(async move |this, cx| {
            loop {
                let next = select_biased! {
                    text = detail_rx.recv_async().fuse() => Some(text),
                    result = rx.recv_async().fuse() => {
                        let result = result.unwrap_or_else(|_| {
                            Err(LocateError::new("the install ended without a result"))
                        });
                        let _ = this.update(cx, |this, cx| this.on_installed(result, cx));
                        return;
                    }
                };
                if let Some(Err(_)) = next {
                    // The install's progress channel closed: only its result is left.
                    let result = rx.recv_async().await.unwrap_or_else(|_| {
                        Err(LocateError::new("the install ended without a result"))
                    });
                    let _ = this.update(cx, |this, cx| this.on_installed(result, cx));
                    return;
                }
                if let Some(Ok(text)) = next {
                    let alive = this
                        .update(cx, |this, cx| {
                            this.detail = text.clone();
                            let events = this.session.starting_detail(&text);
                            this.emit_all(events, cx);
                        })
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
            }
        }));
        cx.notify();
    }

    fn on_installed(&mut self, result: Result<(), LocateError>, cx: &mut Context<Self>) {
        self.installing = false;
        match result {
            Ok(()) => {
                // Not running yet: the status is still the install's "Starting".
                self.session = Session::new(self.spec);
                self.start(cx);
            }
            Err(error) => {
                // Still not installed: the install is offered again.
                let events = self.session.missing(format!(
                    "The install of {} failed: {}",
                    self.spec.display_name, error.message
                ));
                self.emit_all(events, cx);
            }
        }
    }

    /// Adopt new settings. Returns whether the process needs a restart for
    /// them to take effect (the caller decides when).
    pub fn apply_settings(&mut self, settings: ServerSettings) -> bool {
        let restart = self.settings.needs_restart(&settings);
        if let Some(connection) = &self.connection {
            connection.set_trace(settings.trace);
            if settings.trace != self.settings.trace {
                let _ = connection.notify("$/setTrace", json!({ "value": settings.trace.as_str() }));
            }
        }
        self.settings = settings;
        restart
    }

    // ---- documents --------------------------------------------------------

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("{} is not running", self.spec.display_name))?;
        connection.notify(method, params)?;
        Ok(())
    }

    fn send_all(&self, notifications: Notifications) {
        for (method, params) in notifications.0 {
            if let Err(error) = self.notify(method, params) {
                tracing::debug!("[{}] {method} not sent: {error}", self.id.server);
            }
        }
    }

    /// An editor opened `path`. The server sees one `didOpen` however many
    /// editors show the file; before it has answered `initialize` the open
    /// is held and sent when it has.
    pub fn did_open_file(&self, path: &Path, content: &str, language_id: &str) -> Result<()> {
        if !self.is_running() {
            return Err(anyhow!("{} is not running", self.spec.display_name));
        }
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .open(path, content, language_id, self.is_initialized());
        self.send_all(notifications);
        Ok(())
    }

    /// The `languageId` this server uses for `path`.
    pub fn language_id_of(&self, path: &Path) -> &'static str {
        self.language.language_id_of(path)
    }

    pub fn did_change_file(&self, path: &Path, content: &str, version: i32) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .change_full(path, content, version);
        self.send_all(notifications);
        Ok(())
    }

    /// An incremental change (`range` + `text`, in order). `Err` means the
    /// server cannot take it (it only accepts full text, or has not been told
    /// of the document yet): send the whole text with
    /// [`LanguageServer::did_change_file`].
    pub fn did_change_file_incremental(
        &self,
        path: &Path,
        version: i32,
        changes: Vec<Value>,
    ) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }
        if !self.supports_incremental_sync() {
            return Err(anyhow!("the server takes full-text changes only"));
        }
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .change_incremental(path, version, changes)
            .ok_or_else(|| anyhow!("the server has not been told of this document yet"))?;
        self.send_all(notifications);
        Ok(())
    }

    pub fn did_save_file(&self, path: &Path, content: &str) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .saved(path, content);
        self.send_all(notifications);
        Ok(())
    }

    pub fn did_close_file(&self, path: &Path) -> Result<()> {
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .close(path);
        self.send_all(notifications);
        Ok(())
    }

    /// Whether an editor has `path` open here.
    pub fn is_document_open(&self, path: &Path) -> bool {
        self.documents
            .lock()
            .map(|docs| docs.is_open(path))
            .unwrap_or(false)
    }

    /// Whether the server has been told of `path` (so requests about it are
    /// meaningful).
    pub fn is_document_announced(&self, path: &Path) -> bool {
        self.documents
            .lock()
            .map(|docs| docs.is_announced(path))
            .unwrap_or(false)
    }

    /// Record that editors are active (resets the idle clock).
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    // ---- requests ---------------------------------------------------------

    /// Send a request; the receiver yields the whole response message
    /// (`result` or `error`) or disconnects if the server goes away first.
    pub fn send_request_async(&self, method: &str, params: Value) -> Result<flume::Receiver<Value>> {
        if !self.is_initialized() {
            return Err(anyhow!("{} is not running", self.spec.display_name));
        }
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("{} is not running", self.spec.display_name))?;
        Ok(connection.request(method, params)?)
    }

    /// Code actions for a diagnostic, using the raw diagnostic the server
    /// published (it carries the server's own `data`).
    pub fn request_code_actions_with_diagnostic(
        &self,
        file_path: &Path,
        raw_diagnostic: &Value,
    ) -> Result<flume::Receiver<Value>> {
        let range = raw_diagnostic.get("range").cloned().unwrap_or_else(|| {
            json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } })
        });
        self.send_request_async(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": path_to_uri(file_path) },
                "range": range,
                "context": {
                    "diagnostics": [raw_diagnostic],
                    "only": ["quickfix"],
                    "triggerKind": 1
                }
            }),
        )
    }

    /// Resolve a code action (fetch its edits).
    pub fn resolve_code_action_async(&self, action: &Value) -> Result<flume::Receiver<Value>> {
        self.send_request_async("codeAction/resolve", action.clone())
    }

    /// The code actions with edits in a `codeAction` response, and those that
    /// still need resolving.
    pub fn split_code_actions(response: &Value) -> (Vec<ui::diagnostics::CodeAction>, Vec<Value>) {
        let result = response.get("result").unwrap_or(&Value::Null);
        (
            code_actions::parse_code_actions(result),
            code_actions::get_unresolved_actions(result),
        )
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.kill_process();
    }
}

// ---- the server's thread ----------------------------------------------------

/// The body of a server's service thread: locate, spawn, handshake, read.
fn run_server(
    spec: &'static LanguageServerSpec,
    context: Arc<ServerContext>,
    child_slot: Arc<Mutex<Option<Child>>>,
    tx: flume::Sender<Msg>,
    task: &editor_task_queue::TaskContext,
    transport: Transport,
) {
    let label = format!("{}:{}", spec.id, context.root.display());

    task.set_phase("Preparing the workspace");
    if let Some(prepare) = spec.prepare_workspace {
        if let Err(error) = prepare(&context) {
            let _ = tx.send(Msg::Failed(format!("could not prepare the workspace: {error}")));
            return;
        }
    }

    if let Transport::Attached(reader, writer) = transport {
        // Already connected: no locating, no spawning, the same conversation.
        let connection = Connection::new(writer, label);
        connection.set_trace(context.settings.trace);
        let _ = tx.send(Msg::Spawned(connection.clone()));
        serve(spec, context, &tx, task, connection, std::io::BufReader::new(reader));
        task.set_phase("Exited");
        let _ = tx.send(Msg::Exited("closed its connection".to_string()));
        return;
    }

    task.set_phase("Locating the server");
    let detail_tx = tx.clone();
    let progress = move |text: &str| {
        let _ = detail_tx.send(Msg::Detail(text.to_string()));
    };
    let locate = LocateContext::new(&context.project_root, context.settings.path.clone(), &progress);
    let mut command: ServerCommand = match (spec.locate)(&locate) {
        Ok(command) => command,
        Err(error) => {
            let _ = tx.send(if error.missing {
                Msg::Missing(error.message)
            } else {
                Msg::Failed(error.message)
            });
            return;
        }
    };
    command
        .args
        .extend(context.settings.args.iter().map(std::ffi::OsString::from));
    tracing::info!("[{label}] starting {:?} {:?}", command.program, command.args);

    task.set_phase("Spawning the process");
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .envs(command.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&context.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // A console window must never flash for a server.
        process.creation_flags(0x0800_0000);
    }
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = tx.send(Msg::Failed(format!(
                "could not start {} ({}): {error}",
                spec.display_name,
                command.program.display()
            )));
            return;
        }
    };
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        let _ = tx.send(Msg::Failed("the server's pipes are not available".to_string()));
        return;
    };
    if let Ok(mut slot) = child_slot.lock() {
        *slot = Some(child);
    }

    // The server's stderr is its log: it lands in this task's log.
    let span = task.span();
    let stderr_label = label.clone();
    let _ = editor_task_queue::spawn_thread(
        editor_task_queue::ThreadSpec::new(
            format!("lsp stderr: {}", spec.id),
            editor_task_queue::ThreadRole::Io,
        )
        .owner("pulsar_lsp"),
        move || {
            use std::io::BufRead as _;
            span.in_scope(|| {
                for line in std::io::BufReader::new(stderr).lines().map_while(|l| l.ok()) {
                    tracing::info!("[{stderr_label}] {line}");
                }
            });
        },
    );

    let connection = Connection::new(stdin, label.clone());
    connection.set_trace(context.settings.trace);
    let _ = tx.send(Msg::Spawned(connection.clone()));
    serve(spec, context, &tx, task, connection, std::io::BufReader::new(stdout));

    // The stream ended: the process is exiting (or was killed).
    task.set_phase("Exited");
    let how = match child_slot.lock().ok().and_then(|mut slot| slot.take()) {
        Some(mut child) => {
            let status = wait_briefly(&mut child);
            match status {
                Some(status) if status.success() => "exited normally".to_string(),
                Some(status) => format!("exited with {status}"),
                None => "closed its output and was stopped".to_string(),
            }
        }
        // Stopped on purpose: the entity already took the child.
        None => "was stopped".to_string(),
    };
    tracing::info!("[{label}] {how}");
    let _ = tx.send(Msg::Exited(how));
}

/// The conversation with a connected server: the handshake, then reading
/// until the stream ends, answering what the server asks.
fn serve(
    spec: &'static LanguageServerSpec,
    context: Arc<ServerContext>,
    tx: &flume::Sender<Msg>,
    task: &editor_task_queue::TaskContext,
    connection: Arc<Connection>,
    reader: impl std::io::BufRead,
) {
    let label = format!("{}:{}", spec.id, context.root.display());
    task.set_phase("Handshake");
    let init_tx = tx.clone();
    let init_connection = connection.clone();
    let initialize = initialize_params(spec, &context);
    let sent = connection.request_with("initialize", initialize, move |response| {
        if let Some(error) = response.get("error") {
            let _ = init_tx.send(Msg::Failed(format!("initialize failed: {error}")));
            return;
        }
        // `initialized` goes out before anything the entity sends next.
        let _ = init_connection.notify("initialized", json!({}));
        let capabilities = response
            .pointer("/result/capabilities")
            .cloned()
            .unwrap_or(Value::Null);
        let _ = init_tx.send(Msg::Initialized { capabilities });
    });
    if let Err(error) = sent {
        let _ = tx.send(Msg::Failed(format!("could not write to the server: {error}")));
        return;
    }

    task.set_phase("Serving");
    task.set_idle(true);
    let mut handler = ServerHandler {
        spec,
        context,
        tx: tx.clone(),
        connection: connection.clone(),
        task: task.clone(),
        label,
    };
    protocol::run_reader(reader, &connection, &mut handler);
}

/// Wait for a child whose output closed; stop it if it lingers.
fn wait_briefly(child: &mut Child) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn initialize_params(spec: &LanguageServerSpec, context: &ServerContext) -> Value {
    let root_uri = path_to_uri(&context.root);
    let name = context
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "Pulsar", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": root_uri,
        "workspaceFolders": [{ "uri": root_uri, "name": name }],
        "capabilities": capabilities::with_spec((spec.client_capabilities)()),
        "initializationOptions": spec.effective_initialization_options(context),
        "trace": context.settings.trace.as_str(),
    })
}

/// What the reader thread does with what the server sends.
struct ServerHandler {
    spec: &'static LanguageServerSpec,
    context: Arc<ServerContext>,
    tx: flume::Sender<Msg>,
    connection: Arc<Connection>,
    task: editor_task_queue::TaskContext,
    label: String,
}

impl Handler for ServerHandler {
    fn request(&mut self, id: &Value, method: &str, params: &Value) -> RequestOutcome {
        self.task.set_idle(false);
        self.task.heartbeat();
        let outcome = match method {
            // Nothing to do but acknowledge: tsserver and others wait for it.
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "window/showMessageRequest"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/diagnostic/refresh" => RequestOutcome::Reply(Value::Null),
            "workspace/configuration" => {
                let sections: Vec<Value> = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| {
                                let section = item.get("section").and_then(Value::as_str).unwrap_or("");
                                (self.spec.configuration)(&self.context, section)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                RequestOutcome::Reply(Value::Array(sections))
            }
            "workspace/workspaceFolders" => {
                let uri = path_to_uri(&self.context.root);
                let name = self
                    .context
                    .root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                RequestOutcome::Reply(json!([{ "uri": uri, "name": name }]))
            }
            "window/showDocument" => RequestOutcome::Reply(json!({ "success": false })),
            "workspace/applyEdit" => {
                let _ = self.tx.send(Msg::ApplyEdit {
                    id: id.clone(),
                    params: params.clone(),
                });
                RequestOutcome::Deferred
            }
            other => RequestOutcome::Error(
                METHOD_NOT_FOUND,
                format!("Pulsar does not handle `{other}`"),
            ),
        };
        self.task.set_idle(true);
        outcome
    }

    fn notification(&mut self, method: &str, params: Value) {
        self.task.set_idle(false);
        self.task.heartbeat();
        match method {
            "$/progress" => {
                if let Some(message) = parse_progress(&params) {
                    let _ = self.tx.send(message);
                }
            }
            "textDocument/publishDiagnostics" => {
                if let Some((path, diagnostics)) =
                    parse_publish(&params, self.spec.id, &self.spec.hooks)
                {
                    let _ = self.tx.send(Msg::Diagnostics { path, diagnostics });
                }
            }
            "window/logMessage" | "window/showMessage" => {
                let text = params.get("message").and_then(Value::as_str).unwrap_or("");
                match params.get("type").and_then(Value::as_u64) {
                    Some(1) => tracing::error!("[{}] {text}", self.label),
                    Some(2) => tracing::warn!("[{}] {text}", self.label),
                    _ => tracing::info!("[{}] {text}", self.label),
                }
            }
            other => {
                if let Some(signal) = (self.spec.hooks.notification)(other, &params) {
                    let _ = self.tx.send(Msg::Signal(signal));
                } else {
                    tracing::debug!("[{}] unhandled notification {other}", self.label);
                }
            }
        }
        let _ = &self.connection;
        self.task.set_idle(true);
    }
}

fn parse_progress(params: &Value) -> Option<Msg> {
    let token = match params.get("token")? {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let value = params.get("value")?;
    let kind = match value.get("kind")?.as_str()? {
        "begin" => ProgressKind::Begin,
        "report" => ProgressKind::Report,
        "end" => ProgressKind::End,
        _ => return None,
    };
    Some(Msg::Progress {
        token,
        kind,
        title: value.get("title").and_then(Value::as_str).map(str::to_owned),
        message: value.get("message").and_then(Value::as_str).map(str::to_owned),
        percentage: value.get("percentage").and_then(Value::as_u64).map(|p| p as u32),
    })
}

/// Resolve the file a `Location`-shaped answer points at (shared by the
/// editor's command handling).
pub fn location_target(result: &Value) -> Option<(PathBuf, u32, u32)> {
    let item = match result {
        Value::Array(items) => items.first()?,
        Value::Object(_) => result,
        _ => return None,
    };
    let uri = item.get("targetUri").or_else(|| item.get("uri"))?.as_str()?;
    let range = item.get("targetSelectionRange").or_else(|| item.get("range"))?;
    let start = range.get("start")?;
    Some((
        uri_to_path(uri),
        start.get("line")?.as_u64()? as u32,
        start.get("character")?.as_u64()? as u32,
    ))
}

/// The configuration answer for a section when the spec has no opinion:
/// the section of its effective initialization options.
pub fn default_configuration(spec: &LanguageServerSpec, context: &ServerContext, section: &str) -> Value {
    section_of(&spec.effective_initialization_options(context), section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::fake::*;
    use pulsar_language::spec::{LocateError, RootStrategy, ServerHooks};

    fn spec() -> &'static LanguageServerSpec {
        Box::leak(Box::new(LanguageServerSpec {
            id: "fake",
            display_name: "Fake",
            find: |_| None,
            locate: |_| Err(LocateError::new("not used")),
            roots: RootStrategy::ProjectRoot,
            initialization_options: |_| json!({ "section": { "key": 7 } }),
            configuration: |_, section| section_of(&json!({ "section": { "key": 7 } }), section),
            client_capabilities: || Value::Null,
            hooks: ServerHooks::DEFAULT,
            readiness: pulsar_language::spec::ReadinessRule::Immediate,
            max_indexing: Duration::from_secs(5),
            eager: false,
            commands: &[],
            prepare_workspace: None,
            installer: None,
        }))
    }

    struct Fixture {
        _task: editor_task_queue::PersistentTask,
        handler: ServerHandler,
        rx: flume::Receiver<Msg>,
        written: flume::Receiver<Vec<u8>>,
    }

    fn fixture() -> Fixture {
        let (tx, rx) = flume::unbounded();
        let (stdin, written) = flume::unbounded();
        let persistent = editor_task_queue::global().persistent_task(
            editor_task_queue::TaskDescription::persistent("test", "test"),
            None,
        );
        let task = persistent.context().clone();
        let spec = spec();
        Fixture {
            _task: persistent,
            handler: ServerHandler {
                spec,
                context: Arc::new(ServerContext {
                    root: PathBuf::from("/p"),
                    project_root: PathBuf::from("/p"),
                    settings: ServerSettings::default(),
                }),
                tx,
                connection: Connection::new(PipeWriter(stdin), "t"),
                task,
                label: "t".into(),
            },
            rx,
            written,
        }
    }

    #[test]
    fn registration_progress_creation_and_refresh_requests_are_acknowledged_with_null() {
        let mut f = fixture();
        for method in [
            "client/registerCapability",
            "window/workDoneProgress/create",
            "workspace/semanticTokens/refresh",
            "workspace/inlayHint/refresh",
        ] {
            assert_eq!(
                f.handler.request(&json!(1), method, &json!({})),
                RequestOutcome::Reply(Value::Null),
                "{method}"
            );
        }
        let _ = &f.written;
    }

    #[test]
    fn workspace_configuration_is_answered_per_item_from_the_spec() {
        let mut f = fixture();
        let reply = f.handler.request(
            &json!(2),
            "workspace/configuration",
            &json!({ "items": [{ "section": "section.key" }, { "section": "missing" }] }),
        );
        assert_eq!(reply, RequestOutcome::Reply(json!([7, null])));
    }

    #[test]
    fn unknown_requests_are_method_not_found_and_apply_edit_is_forwarded_not_answered() {
        let mut f = fixture();
        assert!(matches!(
            f.handler.request(&json!(3), "x/unknown", &json!({})),
            RequestOutcome::Error(METHOD_NOT_FOUND, _)
        ));
        let outcome = f
            .handler
            .request(&json!("e1"), "workspace/applyEdit", &json!({ "edit": {} }));
        assert_eq!(outcome, RequestOutcome::Deferred);
        match f.rx.try_recv().unwrap() {
            Msg::ApplyEdit { id, .. } => assert_eq!(id, json!("e1")),
            _ => panic!("expected the edit to be forwarded to the UI thread"),
        }
    }

    #[test]
    fn progress_and_diagnostics_notifications_are_decoded_for_the_entity() {
        let mut f = fixture();
        f.handler.notification(
            "$/progress",
            json!({ "token": 12, "value": { "kind": "begin", "title": "Indexing", "percentage": 0 } }),
        );
        match f.rx.try_recv().unwrap() {
            Msg::Progress { token, kind, title, .. } => {
                assert_eq!(token, "12");
                assert_eq!(kind, ProgressKind::Begin);
                assert_eq!(title.as_deref(), Some("Indexing"));
            }
            _ => panic!("expected progress"),
        }
        f.handler.notification(
            "textDocument/publishDiagnostics",
            json!({ "uri": "file:///p/a.fk", "diagnostics": [] }),
        );
        match f.rx.try_recv().unwrap() {
            Msg::Diagnostics { path, diagnostics } => {
                assert_eq!(path, PathBuf::from("/p/a.fk"));
                assert!(diagnostics.is_empty(), "an empty publish is delivered so it can clear");
            }
            _ => panic!("expected diagnostics"),
        }
    }

    #[test]
    fn a_location_answer_decodes_to_a_path_and_position() {
        let result = json!([{ "targetUri": "file:///p/a.rs", "targetSelectionRange": {
            "start": { "line": 4, "character": 2 }, "end": { "line": 4, "character": 5 }
        }}]);
        assert_eq!(location_target(&result), Some((PathBuf::from("/p/a.rs"), 4, 2)));
    }

    /// A scripted server on a pipe: the handshake, then the server's own
    /// requests (one with an id that collides with a pending client id),
    /// progress, an edit request, and the end of the stream.
    #[test]
    fn a_scripted_server_handshakes_is_answered_and_forwards_what_the_ui_needs() {
        use crate::protocol::read_message;
        let mut wire = wire();
        let (tx, rx) = flume::unbounded();
        let persistent = editor_task_queue::global().persistent_task(
            editor_task_queue::TaskDescription::persistent("scripted", "test"),
            None,
        );
        let task = persistent.context().clone();
        let context = Arc::new(ServerContext {
            root: PathBuf::from("/p"),
            project_root: PathBuf::from("/p"),
            settings: ServerSettings::default(),
        });
        let connection = wire.connection.clone();
        let reader = wire.client_reader.take().unwrap();
        let serving = std::thread::spawn(move || {
            serve(spec(), context, &tx, &task, connection, reader);
        });

        // 1. The client's `initialize` (id 1).
        let initialize = read_message(&mut wire.from_client).unwrap().unwrap();
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["params"]["rootUri"], "file:///p");
        assert_eq!(initialize["params"]["capabilities"]["workspace"]["applyEdit"], true);
        send(&wire.to_client, json!({
            "jsonrpc": "2.0", "id": initialize["id"],
            "result": { "capabilities": { "completionProvider": { "triggerCharacters": ["."] } } }
        }));
        // 2. `initialized` follows, then the entity hears of the capabilities.
        let initialized = read_message(&mut wire.from_client).unwrap().unwrap();
        assert_eq!(initialized["method"], "initialized");
        match rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
            Msg::Initialized { capabilities } => {
                assert_eq!(capabilities["completionProvider"]["triggerCharacters"][0], ".");
            }
            _ => panic!("expected the capabilities"),
        }

        // 3. The server asks for things with ids that collide with ours: both
        //    are answered, and neither is mistaken for a response.
        let request = wire.connection.request("textDocument/hover", json!({})).unwrap();
        let ours = read_message(&mut wire.from_client).unwrap().unwrap();
        send(&wire.to_client, json!({
            "jsonrpc": "2.0", "id": ours["id"], "method": "client/registerCapability", "params": {}
        }));
        let reply = read_message(&mut wire.from_client).unwrap().unwrap();
        assert_eq!(reply["id"], ours["id"]);
        assert_eq!(reply["result"], Value::Null);
        send(&wire.to_client, json!({
            "jsonrpc": "2.0", "id": 77, "method": "workspace/configuration",
            "params": { "items": [{ "section": "section.key" }] }
        }));
        let reply = read_message(&mut wire.from_client).unwrap().unwrap();
        assert_eq!(reply["id"], 77);
        assert_eq!(reply["result"], json!([7]));
        assert!(request.try_recv().is_err(), "still waiting for its own response");
        send(&wire.to_client, json!({ "jsonrpc": "2.0", "id": ours["id"], "result": "hovered" }));
        assert_eq!(request.recv_timeout(std::time::Duration::from_secs(5)).unwrap()["result"], "hovered");

        // 4. Progress and an edit request reach the UI side; the edit request
        //    is answered later, by the UI.
        send(&wire.to_client, json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "t", "value": { "kind": "end" } }
        }));
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            Msg::Progress { kind: ProgressKind::End, .. }
        ));
        send(&wire.to_client, json!({
            "jsonrpc": "2.0", "id": "edit-1", "method": "workspace/applyEdit", "params": { "edit": {} }
        }));
        let Msg::ApplyEdit { id, .. } = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() else {
            panic!("expected the edit request");
        };
        wire.connection.respond(&id, Ok(json!({ "applied": true }))).unwrap();
        let reply = read_message(&mut wire.from_client).unwrap().unwrap();
        assert_eq!(reply["id"], "edit-1");
        assert_eq!(reply["result"]["applied"], true);

        // 5. The stream ends: the reader returns and the connection is closed.
        drop(wire.to_client);
        serving.join().unwrap();
        assert!(wire.connection.is_closed());
        drop(persistent);
    }
}
