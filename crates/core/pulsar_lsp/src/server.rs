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

mod client;
mod handler;
mod install;
mod process;

#[cfg(test)]
mod tests;

pub use handler::{default_configuration, location_target};

use handler::ServerHandler;
use process::run_server;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::{select_biased, FutureExt as _};
use gpui::{Context, Entity, EventEmitter, Task};
use serde_json::{json, Value};

use crate::diagnostics::{DiagnosticSource, DiagnosticsStore, ServerId};
use crate::documents::Documents;
use crate::edit::{apply_workspace_edit_in_app, WorkspaceEdit};
use crate::protocol::Connection;
use crate::session::{ServerEvent, ServerStatus, Session, StopReason};
use pulsar_language::settings::{ServerSettings, TraceLevel};
use pulsar_language::spec::{
    LanguageServerSpec, ProgressKind, ServerCommandSpec, ServerContext, ServerSignal,
};
use pulsar_language::Language;

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
    Attached(
        Box<dyn std::io::Read + Send>,
        Box<dyn std::io::Write + Send>,
    ),
}

/// What the server's thread tells the entity.
enum Msg {
    /// Something the footer can show while starting ("Installing ...").
    Detail(String),
    Spawned(Arc<Connection>),
    Initialized {
        capabilities: Value,
    },
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
    Diagnostics {
        path: PathBuf,
        diagnostics: Vec<ui::diagnostics::Diagnostic>,
    },
    ApplyEdit {
        id: Value,
        params: Value,
    },
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
            id: ServerId {
                server: spec.id,
                root,
            },
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
        let kind = sync.and_then(|sync| {
            sync.as_u64()
                .or_else(|| sync.get("change").and_then(Value::as_u64))
        });
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
        let stopped = events
            .iter()
            .any(|event| matches!(event, ServerEvent::Error(_) | ServerEvent::Stopped(_)));
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
                let events = self
                    .session
                    .failed(format!("could not start a thread: {error}"));
                self.emit_all(events, cx);
                return;
            }
        }
        drop(tx);

        // The pump: apply what the thread sends, and tick the caps.
        self._pump = Some(cx.spawn(async move |this, cx| loop {
            let next = select_biased! {
                message = rx.recv_async().fuse() => Some(message),
                _ = cx.background_executor().timer(TICK).fuse() => None,
            };
            let alive = match next {
                Some(Ok(message)) => this
                    .update(cx, |this, cx| this.on_message(generation, message, cx))
                    .is_ok(),
                Some(Err(_)) => break,
                None => this
                    .update(cx, |this, cx| this.on_tick(generation, cx))
                    .is_ok(),
            };
            if !alive {
                break;
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
                let flushed = self
                    .documents
                    .lock()
                    .map(|mut docs| docs.flush())
                    .unwrap_or_default();
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
            Msg::Progress {
                token,
                kind,
                title,
                message,
                percentage,
            } => {
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
                        tracing::warn!(
                            "[{}] workspace/applyEdit refused: {reason}",
                            self.id.server
                        );
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

    /// Adopt new settings. Returns whether the process needs a restart for
    /// them to take effect (the caller decides when).
    pub fn apply_settings(&mut self, settings: ServerSettings) -> bool {
        let restart = self.settings.needs_restart(&settings);
        if let Some(connection) = &self.connection {
            connection.set_trace(settings.trace);
            if settings.trace != self.settings.trace {
                let _ =
                    connection.notify("$/setTrace", json!({ "value": settings.trace.as_str() }));
            }
        }
        self.settings = settings;
        restart
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.kill_process();
    }
}
