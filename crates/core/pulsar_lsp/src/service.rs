//! `LanguageServers`: the service editors and the shell use.
//!
//! It owns every [`LanguageServer`], one per `(server, root)`, and the shared
//! [`DiagnosticsStore`]. A file is routed to its server by `server_for`: the
//! language the registry (`pulsar_language::LanguageRegistry`) says owns the
//! file's extension, that language's server spec, the root its `RootStrategy`
//! picks, created on first use. Servers that say `eager` (and a project that
//! has their language's config files) start when the project opens; every
//! other starts on its first file, unless its `start` setting says
//! otherwise. A server stops when it is switched off in its settings, when its
//! plugin is disabled (its language leaves the registry), when the project
//! changes, or when it has had no open file for its idle period.

mod checks;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{AppContext as _, Context, Entity, EventEmitter, Subscription, Task};
use pulsar_language::settings::{ServerSettings, StartSetting};
use pulsar_language::{Language, LanguageRegistry, LanguageServerSpec};

use crate::diagnostics::{DiagnosticsStore, ServerId};
use crate::server::LanguageServer;
use crate::session::{ServerEvent, ServerStatus, StopReason};

/// How often the service looks at settings changes, the plugin set and idle
/// servers.
const TICK: Duration = Duration::from_secs(2);

/// What the service tells its subscribers: servers coming and going, and
/// every server's own events tagged with its id.
#[derive(Clone, Debug, PartialEq)]
pub enum LanguageServersEvent {
    Added(ServerId),
    Removed(ServerId),
    Server { id: ServerId, event: ServerEvent },
}

pub use checks::{compiler_diagnostics, COMPILER_SOURCE};

pub struct LanguageServers {
    project_root: Option<PathBuf>,
    /// Live compiler checks of open files (`check_source`).
    checks: checks::Checks,
    servers: Vec<Entity<LanguageServer>>,
    diagnostics: Entity<DiagnosticsStore>,
    _subscriptions: HashMap<ServerId, Subscription>,
    settings_tx: flume::Sender<&'static str>,
    settings_rx: flume::Receiver<&'static str>,
    /// One listener per spec on its plugin settings; attached once the group
    /// is registered.
    setting_watches: HashMap<&'static str, pulsar_settings::plugin::Subscription>,
    plugin_generation: u64,
    _tick: Option<Task<()>>,
}

impl EventEmitter<LanguageServersEvent> for LanguageServers {}

/// The languages with a server whose plugin is enabled now, with their
/// specs (what the loading screen checks).
pub fn active_servers() -> Vec<(&'static Language, &'static LanguageServerSpec)> {
    LanguageRegistry::current().with_servers().collect()
}

/// Every linked language with a server, whatever its plugin's state (the
/// settings of a disabled plugin's server are still watched, so they work
/// again when it is enabled).
fn known_servers() -> Vec<(&'static Language, &'static LanguageServerSpec)> {
    pulsar_plugins::all_registered::<Language>()
        .filter_map(|language| language.server.map(|spec| (language, spec)))
        .collect()
}

impl LanguageServers {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let diagnostics = cx.new(|_| DiagnosticsStore::new());
        let (settings_tx, settings_rx) = flume::unbounded();
        let tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                break;
            }
        });
        Self {
            project_root: None,
            checks: checks::Checks::default(),
            servers: Vec::new(),
            diagnostics,
            _subscriptions: HashMap::new(),
            settings_tx,
            settings_rx,
            setting_watches: HashMap::new(),
            plugin_generation: pulsar_plugins::generation(),
            _tick: Some(tick),
        }
    }

    pub fn diagnostics(&self) -> Entity<DiagnosticsStore> {
        self.diagnostics.clone()
    }

    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// Every server this service has created, running or not.
    pub fn servers(&self) -> &[Entity<LanguageServer>] {
        &self.servers
    }

    /// The server with `id`.
    pub fn server(&self, id: &ServerId, cx: &gpui::App) -> Option<Entity<LanguageServer>> {
        self.servers
            .iter()
            .find(|server| server.read(cx).id() == id)
            .cloned()
    }

    /// The language and server spec that handle `path`: what the language
    /// registry says owns its extension, when that language has a server.
    pub fn server_spec_for(&self, path: &Path) -> Option<(&'static Language, &'static LanguageServerSpec)> {
        LanguageRegistry::current().server_for_path(path)
    }

    /// The server that handles `path` if it exists already; never creates or
    /// starts one.
    pub fn existing_server_for(&self, path: &Path, cx: &gpui::App) -> Option<Entity<LanguageServer>> {
        let project_root = self.project_root.as_ref()?;
        let (_, spec) = self.server_spec_for(path)?;
        let root = spec.roots.root_for(path, project_root)?;
        let id = ServerId { server: spec.id, root };
        self.server(&id, cx)
    }

    /// The server that handles `path`, created (and started, the first time)
    /// on demand. `None`: no server claims the file, it is switched off, or
    /// no project is open.
    pub fn server_for(&mut self, path: &Path, cx: &mut Context<Self>) -> Option<Entity<LanguageServer>> {
        let project_root = self.project_root.clone()?;
        let (language, spec) = self.server_spec_for(path)?;
        let settings = ServerSettings::read(language.origin, spec.id);
        if !settings.enabled {
            return None;
        }
        let root = spec.roots.root_for(path, &project_root)?;
        let server = self.ensure_server(language, spec, root, settings, cx);
        server.update(cx, |server, cx| {
            server.touch();
            if matches!(server.status(), ServerStatus::Idle) {
                server.start(cx);
            }
        });
        Some(server)
    }

    fn ensure_server(
        &mut self,
        language: &'static Language,
        spec: &'static LanguageServerSpec,
        root: PathBuf,
        settings: ServerSettings,
        cx: &mut Context<Self>,
    ) -> Entity<LanguageServer> {
        let id = ServerId { server: spec.id, root: root.clone() };
        if let Some(existing) = self.server(&id, cx) {
            return existing;
        }
        let project_root = self.project_root.clone().unwrap_or_else(|| root.clone());
        let diagnostics = self.diagnostics.clone();
        let server = cx.new(|_| LanguageServer::new(language, spec, root, project_root, settings, diagnostics));
        let tag = id.clone();
        let subscription = cx.subscribe(&server, move |_this, _server, event: &ServerEvent, cx| {
            cx.emit(LanguageServersEvent::Server { id: tag.clone(), event: event.clone() });
        });
        self._subscriptions.insert(id.clone(), subscription);
        self.servers.push(server.clone());
        self.watch_settings(language, spec);
        cx.emit(LanguageServersEvent::Added(id));
        cx.notify();
        server
    }

    /// Open a project: the previous project's servers stop, and the eager
    /// ones of this one start.
    pub fn set_project(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.project_root.as_deref() == Some(root.as_path()) {
            return;
        }
        self.remove_all(StopReason::ProjectChanged, cx);
        self.drop_all_checks(cx);
        self.project_root = Some(root.clone());
        for (language, spec) in active_servers() {
            let settings = ServerSettings::read(language.origin, spec.id);
            if !settings.enabled {
                continue;
            }
            let eager = match settings.start {
                StartSetting::Project => true,
                StartSetting::FirstFile => false,
                StartSetting::Auto => spec.eager,
            };
            if !eager || !project_has_config(language, &root) {
                continue;
            }
            let Some(server_root) = spec.roots.root_for(&root.join("_"), &root) else {
                continue;
            };
            let server = self.ensure_server(language, spec, server_root, settings, cx);
            server.update(cx, |server, cx| {
                if matches!(server.status(), ServerStatus::Idle) {
                    server.start(cx);
                }
            });
        }
    }

    fn remove_all(&mut self, reason: StopReason, cx: &mut Context<Self>) {
        for server in std::mem::take(&mut self.servers) {
            let id = server.read(cx).id().clone();
            server.update(cx, |server, cx| server.stop(reason, cx));
            self._subscriptions.remove(&id);
            cx.emit(LanguageServersEvent::Removed(id));
        }
        cx.notify();
    }

    /// Start `id` (a stopped or failed server, from the footer).
    pub fn start(&mut self, id: &ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.server(id, cx) {
            server.update(cx, |server, cx| {
                server.touch();
                server.start(cx)
            });
        }
    }

    /// Run `id`'s managed install (the person chose it in the footer), then
    /// start it. A no-op for a server whose spec offers none.
    pub fn install(&mut self, id: &ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.server(id, cx) {
            server.update(cx, |server, cx| {
                server.touch();
                server.install(cx)
            });
        }
    }

    pub fn stop(&mut self, id: &ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.server(id, cx) {
            server.update(cx, |server, cx| server.stop(StopReason::Requested, cx));
        }
    }

    pub fn restart(&mut self, id: &ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.server(id, cx) {
            server.update(cx, |server, cx| {
                server.touch();
                server.restart(cx)
            });
        }
    }

    /// Listen for changes to the plugin settings of `spec` (once its group
    /// is registered).
    fn watch_settings(&mut self, language: &'static Language, spec: &'static LanguageServerSpec) {
        if self.setting_watches.contains_key(spec.id) {
            return;
        }
        let Some(handle) = pulsar_settings::plugin::handle(language.origin, spec.id) else {
            return;
        };
        let tx = self.settings_tx.clone();
        let id = spec.id;
        let subscription = handle.on_any_change(move |_| {
            let _ = tx.send(id);
        });
        self.setting_watches.insert(spec.id, subscription);
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        // A spec whose settings group was not registered yet when its server
        // was created.
        for (language, spec) in known_servers() {
            self.watch_settings(language, spec);
        }

        // The plugin set changed: a server of a disabled plugin stops and
        // goes, taking its diagnostics.
        let generation = pulsar_plugins::generation();
        if generation != self.plugin_generation {
            self.plugin_generation = generation;
            self.drop_disabled(cx);
            // Settings groups were registered or withdrawn: listen again.
            self.setting_watches.clear();
            for (language, spec) in known_servers() {
                self.watch_settings(language, spec);
            }
        }

        let mut changed: Vec<&'static str> = Vec::new();
        while let Ok(spec) = self.settings_rx.try_recv() {
            if !changed.contains(&spec) {
                changed.push(spec);
            }
        }
        for spec in changed {
            self.apply_settings_of(spec, cx);
        }

        self.checks_tick(cx);
        self.stop_idle(cx);
    }

    fn drop_disabled(&mut self, cx: &mut Context<Self>) {
        let (keep, drop): (Vec<_>, Vec<_>) = std::mem::take(&mut self.servers)
            .into_iter()
            .partition(|server| pulsar_plugins::origin_enabled(server.read(cx).language().origin));
        self.servers = keep;
        for server in drop {
            let id = server.read(cx).id().clone();
            server.update(cx, |server, cx| server.stop(StopReason::Disabled, cx));
            self._subscriptions.remove(&id);
            cx.emit(LanguageServersEvent::Removed(id));
        }
        cx.notify();
    }

    fn apply_settings_of(&mut self, spec_id: &'static str, cx: &mut Context<Self>) {
        let Some((language, spec)) = known_servers().into_iter().find(|(_, spec)| spec.id == spec_id) else {
            return;
        };
        let settings = ServerSettings::read(language.origin, spec.id);
        let servers: Vec<_> = self
            .servers
            .iter()
            .filter(|server| server.read(cx).id().server == spec_id)
            .cloned()
            .collect();
        for server in servers {
            let settings = settings.clone();
            server.update(cx, |server, cx| {
                let needs_restart = server.apply_settings(settings.clone());
                if !settings.enabled {
                    if server.is_running() {
                        server.stop(StopReason::Disabled, cx);
                    }
                } else if server.is_running() && needs_restart {
                    server.restart(cx);
                } else if !server.is_running()
                    && matches!(server.status(), ServerStatus::Stopped)
                    && settings.enabled
                {
                    // Switched back on: it comes back when a file needs it
                    // (or now, when it is an eager one).
                    if server.spec().eager || settings.start == StartSetting::Project {
                        server.start(cx);
                    }
                }
            });
        }
    }

    fn stop_idle(&mut self, cx: &mut Context<Self>) {
        for server in self.servers.clone() {
            let idle = {
                let server = server.read(cx);
                let minutes = server.settings().idle_stop_minutes;
                minutes > 0
                    && server.is_running()
                    && server.open_document_count() == 0
                    && server.last_activity().elapsed() >= Duration::from_secs(minutes as u64 * 60)
            };
            if idle {
                server.update(cx, |server, cx| server.stop(StopReason::Idle, cx));
            }
        }
    }
}

/// Whether the project has what the spec's server works on (its config
/// files); a spec with none always does.
fn project_has_config(language: &Language, project_root: &Path) -> bool {
    language.config_files.is_empty()
        || language
            .config_files
            .iter()
            .any(|file| engine_fs::virtual_fs::is_file(&project_root.join(file)))
}

impl Drop for LanguageServers {
    fn drop(&mut self) {
        // Each server's own Drop kills its process.
        self.servers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_language::spec::{
        LocateContext, LocateError, ReadinessRule, RootStrategy, ServerCommand, ServerContext,
        ServerHooks,
    };

    /// A language with a server, registered as an engine language (always
    /// enabled): the service names no real one, so its tests bring their own.
    static SPEC: pulsar_language::LanguageServerSpec = pulsar_language::LanguageServerSpec {
        id: "test-server",
        display_name: "test server",
        find: |_: &LocateContext| None,
        locate: |_: &LocateContext| -> Result<ServerCommand, LocateError> {
            Err(LocateError::new("a test server cannot be started"))
        },
        roots: RootStrategy::OutermostMarker(&["workspace.tl"]),
        initialization_options: |_: &ServerContext| serde_json::Value::Null,
        configuration: |_: &ServerContext, _: &str| serde_json::Value::Null,
        client_capabilities: || serde_json::Value::Null,
        hooks: ServerHooks::DEFAULT,
        readiness: ReadinessRule::Signal,
        max_indexing: std::time::Duration::from_secs(1),
        eager: true,
        commands: &[],
        prepare_workspace: None,
        installer: None,
    };

    const LANGUAGE: Language = Language {
        origin: pulsar_plugins::engine_origin!(),
        id: "test-lang",
        display_name: "Test language",
        file_extensions: &["tl"],
        language_ids: &[],
        highlight_id: "",
        class_markers: &[],
        config_files: &["workspace.tl"],
        compiler: None,
        server: Some(&SPEC),
    };

    pulsar_plugins::inventory::submit! { LANGUAGE }

    #[test]
    fn files_of_a_language_with_a_server_have_a_spec_and_others_do_not() {
        let spec = LanguageRegistry::current().server_for_path(Path::new("/p/src/main.tl"));
        assert_eq!(spec.map(|(_, spec)| spec.id), Some("test-server"));
        assert!(LanguageRegistry::current().server_for_path(Path::new("/p/readme.md")).is_none());
    }

    #[test]
    fn eager_servers_need_their_config_files() {
        let root = std::env::temp_dir().join(format!("pulsar_lsp_cfg_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(!project_has_config(&LANGUAGE, &root));
        std::fs::write(root.join("workspace.tl"), "").unwrap();
        assert!(project_has_config(&LANGUAGE, &root));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn temp_project(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pulsar_lsp_svc_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn diagnostic(file: &Path) -> ui::diagnostics::Diagnostic {
        ui::diagnostics::Diagnostic {
            file_path: file.to_string_lossy().into_owned(),
            line: 1,
            column: 1,
            end_line: None,
            end_column: None,
            severity: ui::diagnostics::DiagnosticSeverity::Error,
            message: "bad".into(),
            code: None,
            source: None,
            code_actions: Vec::new(),
            raw_lsp_diagnostic: None,
        }
    }

    /// Per-file routing by `(server, root)`: files under two workspaces that
    /// each have their own outermost manifest reach two servers; a file the
    /// spec does not claim reaches none. Nothing is started (the processes
    /// are not the subject here).
    #[gpui::test]
    fn files_are_routed_to_the_server_of_their_root(cx: &mut gpui::TestAppContext) {
        let project = temp_project("routing");
        // Two sibling workspaces: each has a manifest, the project root has
        // none, so each is the outermost marker for its own files.
        for name in ["one", "two"] {
            std::fs::create_dir_all(project.join(name).join("src")).unwrap();
            std::fs::write(project.join(name).join("workspace.tl"), "[workspace]").unwrap();
        }
        let service = cx.new(LanguageServers::new);
        service.update(cx, |service, cx| {
            service.project_root = Some(project.clone());
            let (language, spec) = (&LANGUAGE, &SPEC);
            for name in ["one", "two"] {
                let file = project.join(name).join("src").join("lib.tl");
                let root = spec.roots.root_for(&file, &project).unwrap();
                service.ensure_server(language, spec, root, ServerSettings::default(), cx);
            }
        });
        service.read_with(cx, |service, cx| {
            assert_eq!(service.servers().len(), 2);
            let one = service
                .existing_server_for(&project.join("one").join("src").join("lib.tl"), cx)
                .unwrap();
            let two = service
                .existing_server_for(&project.join("two").join("src").join("lib.tl"), cx)
                .unwrap();
            assert_ne!(one.entity_id(), two.entity_id());
            assert_eq!(one.read(cx).root(), project.join("one"));
            assert!(service.existing_server_for(&project.join("one").join("notes.md"), cx).is_none());
        });
        let _ = std::fs::remove_dir_all(&project);
    }

    /// A stopped server takes its diagnostics with it, and announces each
    /// file so editors and the drawer clear (diagnostics clear on stop).
    #[gpui::test]
    fn stopping_a_server_clears_its_diagnostics_and_only_its_own(cx: &mut gpui::TestAppContext) {
        let project = temp_project("clear");
        let service = cx.new(LanguageServers::new);
        let file = project.join("src").join("lib.tl");
        let (server, store) = service.update(cx, |service, cx| {
            service.project_root = Some(project.clone());
            let server = service.ensure_server(&LANGUAGE, &SPEC, project.clone(), ServerSettings::default(), cx);
            (server, service.diagnostics())
        });
        let id = server.read_with(cx, |server, _| server.id().clone());
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&store, move |_, event: &crate::DiagnosticsEvent, _| {
                seen.borrow_mut().push(event.clone());
            })
        });
        store.update(cx, |store, cx| {
            store.set(crate::DiagnosticSource::Server(id.clone()), &file, vec![diagnostic(&file)], cx);
            store.set(crate::DiagnosticSource::Compiler("typescript"), &file, vec![diagnostic(&file)], cx);
        });
        assert_eq!(store.read_with(cx, |store, _| store.for_file(&file).len()), 2);
        events.borrow_mut().clear();

        server.update(cx, |server, cx| server.stop(StopReason::Requested, cx));

        let left = store.read_with(cx, |store, _| store.for_file(&file));
        assert_eq!(left.len(), 1, "only the compiler's entry remains");
        assert!(matches!(left[0].0, crate::DiagnosticSource::Compiler("typescript")));
        assert_eq!(events.borrow().len(), 1, "the file is announced so views clear");
        assert_eq!(server.read_with(cx, |server, _| server.status().clone()), ServerStatus::Stopped);
        let _ = std::fs::remove_dir_all(&project);
    }

    /// Switching a server off in its settings stops it; the stop reason is
    /// carried in the event the footer shows.
    #[gpui::test]
    fn a_stop_is_announced_with_its_reason(cx: &mut gpui::TestAppContext) {
        let project = temp_project("reason");
        let service = cx.new(LanguageServers::new);
        let server = service.update(cx, |service, cx| {
            service.project_root = Some(project.clone());
            service.ensure_server(&LANGUAGE, &SPEC, project.clone(), ServerSettings::default(), cx)
        });
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&service, move |_, event: &LanguageServersEvent, _| {
                seen.borrow_mut().push(event.clone());
            })
        });
        server.update(cx, |server, cx| server.stop(StopReason::Disabled, cx));
        assert!(events.borrow().iter().any(|event| matches!(
            event,
            LanguageServersEvent::Server { event: ServerEvent::Stopped(StopReason::Disabled), .. }
        )));
        let _ = std::fs::remove_dir_all(&project);
    }

    // ---- a managed install is offered, never run on its own ----------------

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static INSTALLS: AtomicUsize = AtomicUsize::new(0);
    static INSTALL_FAILS: AtomicBool = AtomicBool::new(true);

    static INSTALLER: pulsar_language::ServerInstaller = pulsar_language::ServerInstaller {
        label: "Install the installable server",
        describe: || "Downloads the installable server".to_string(),
        install: |ctx| {
            ctx.report("Downloading the installable server...");
            INSTALLS.fetch_add(1, Ordering::SeqCst);
            if INSTALL_FAILS.load(Ordering::SeqCst) {
                Err(LocateError::new("the network is offline"))
            } else {
                Ok(())
            }
        },
    };

    static INSTALLABLE: pulsar_language::LanguageServerSpec = pulsar_language::LanguageServerSpec {
        id: "installable-server",
        display_name: "Installable server",
        find: |_: &LocateContext| None,
        locate: |_: &LocateContext| -> Result<ServerCommand, LocateError> {
            Err(LocateError::missing("The installable server is not installed."))
        },
        roots: RootStrategy::ProjectRoot,
        initialization_options: |_: &ServerContext| serde_json::Value::Null,
        configuration: |_: &ServerContext, _: &str| serde_json::Value::Null,
        client_capabilities: || serde_json::Value::Null,
        hooks: ServerHooks::DEFAULT,
        readiness: ReadinessRule::Immediate,
        max_indexing: std::time::Duration::from_secs(1),
        eager: false,
        commands: &[],
        prepare_workspace: None,
        installer: Some(&INSTALLER),
    };

    const INSTALLABLE_LANGUAGE: Language = Language {
        origin: pulsar_plugins::engine_origin!(),
        id: "installable-lang",
        display_name: "Installable language",
        file_extensions: &["ins"],
        language_ids: &[],
        highlight_id: "",
        class_markers: &[],
        config_files: &[],
        compiler: None,
        server: Some(&INSTALLABLE),
    };

    pulsar_plugins::inventory::submit! { INSTALLABLE_LANGUAGE }

    fn wait_until(cx: &mut gpui::TestAppContext, what: &str, mut done: impl FnMut(&mut gpui::TestAppContext) -> bool) {
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

    /// A server that is not installed is a Missing status with the spec's
    /// words (and an error event for the toast). Starting it again never runs
    /// the install: only the person's explicit action does, as a task, and a
    /// failed install is reported and offered again.
    #[gpui::test]
    fn a_missing_server_offers_its_install_and_runs_it_only_when_asked(cx: &mut gpui::TestAppContext) {
        let project = temp_project("install");
        let service = cx.new(LanguageServers::new);
        let server = service.update(cx, |service, cx| {
            service.project_root = Some(project.clone());
            service.ensure_server(
                &INSTALLABLE_LANGUAGE,
                &INSTALLABLE,
                project.clone(),
                ServerSettings::default(),
                cx,
            )
        });
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&server, move |_, event: &ServerEvent, _| seen.borrow_mut().push(event.clone()))
        });
        let status = |cx: &mut gpui::TestAppContext| server.read_with(cx, |server, _| server.status().clone());

        INSTALLS.store(0, Ordering::SeqCst);
        INSTALL_FAILS.store(true, Ordering::SeqCst);
        server.update(cx, |server, cx| server.start(cx));
        wait_until(cx, "the missing status", |cx| matches!(status(cx), ServerStatus::Missing(_)));
        assert_eq!(
            status(cx),
            ServerStatus::Missing("The installable server is not installed.".into())
        );
        assert!(events
            .borrow()
            .iter()
            .any(|event| matches!(event, ServerEvent::Error(m) if m.contains("not installed"))));
        assert_eq!(INSTALLS.load(Ordering::SeqCst), 0, "starting never installs");
        assert!(server.read_with(cx, |server, _| server.installer().is_some()));

        // The person chooses Install; it fails: reported, and offered again.
        service.update(cx, |service, cx| service.install(&server.read(cx).id().clone(), cx));
        wait_until(cx, "the failed install", |cx| {
            matches!(status(cx), ServerStatus::Missing(m) if m.contains("The install of Installable server failed"))
        });
        let ServerStatus::Missing(message) = status(cx) else { unreachable!() };
        assert!(message.contains("the network is offline"), "{message}");
        assert_eq!(INSTALLS.load(Ordering::SeqCst), 1);

        // It works the second time: the server is started afterwards (here it
        // is still not located, so it is missing again, but the install ran).
        INSTALL_FAILS.store(false, Ordering::SeqCst);
        service.update(cx, |service, cx| service.install(&server.read(cx).id().clone(), cx));
        wait_until(cx, "the second install", |_| INSTALLS.load(Ordering::SeqCst) == 2);
        wait_until(cx, "the start after the install", |cx| {
            matches!(status(cx), ServerStatus::Missing(m) if m == "The installable server is not installed.")
        });
        assert_eq!(INSTALLS.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(&project);
    }
}
