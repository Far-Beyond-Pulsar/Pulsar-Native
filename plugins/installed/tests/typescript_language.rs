//! The TypeScript plugin's language support as the editor assembles it
//! (P5 of `.agents/LANGUAGE_PLUGINS_PLAN.md`): the real plugin set, the
//! `LanguageServers` service, both diagnostic sources.
//!
//! * disabling `pulsar.typescript` clears both sources (the server's and the
//!   Pulsar compiler's) and removes the server and its settings;
//! * the plugin's own setting (`compiler_diagnostics`) turns the compiler's
//!   source off and on, and the open file is checked again when it is back on.
//!
//! Nothing is spawned: the server's `path` setting names a file that does not
//! exist, so locating it fails (a missing status) and no process starts.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, TestAppContext};
use pulsar_lsp::{DiagnosticSource, LanguageServers};

use installed_plugins as _;

const TYPESCRIPT: &str = "pulsar.typescript";
const SERVER_GROUP: &str = "pulsar.typescript/typescript-language-server";

/// A class the language server accepts and the script subset rejects.
const ARROW: &str = r#"
export default class Door extends ScriptClass {
    opened: int = 0;
    open(): void { const twice = (x: number) => x * 2; this.opened += 1; }
}
"#;

/// The tests change the process's activated plugins and settings: one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn activate(off: &[&str]) {
    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default().with(off, false),
    ));
    pulsar_settings::plugin::sync(engine_state::global_config());
}

fn wait_for(cx: &mut TestAppContext, what: &str, mut done: impl FnMut(&mut TestAppContext) -> bool) {
    cx.executor().allow_parking();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn diagnostic(file: &std::path::Path, message: &str) -> ui::diagnostics::Diagnostic {
    ui::diagnostics::Diagnostic {
        file_path: file.to_string_lossy().into_owned(),
        line: 1,
        column: 1,
        end_line: None,
        end_column: None,
        severity: ui::diagnostics::DiagnosticSeverity::Error,
        message: message.into(),
        code: None,
        source: None,
        code_actions: Vec::new(),
        raw_lsp_diagnostic: None,
    }
}

struct Setup {
    _project: tempfile::TempDir,
    class: std::path::PathBuf,
    service: gpui::Entity<LanguageServers>,
    store: gpui::Entity<pulsar_lsp::DiagnosticsStore>,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    static BOUND: std::sync::Once = std::sync::Once::new();
    BOUND.call_once(|| pulsar_settings::register_all_settings(engine_state::global_config()));
    activate(&[]);
    engine_state::global_config()
        .owner_handle(pulsar_settings::plugin::NS, SERVER_GROUP)
        .expect("the server group is registered while the plugin is enabled")
        .set("path", "/pulsar-test/no-such-typescript-language-server/cli.mjs")
        .unwrap();
    let handle = engine_state::global_config()
        .owner_handle(pulsar_settings::plugin::NS, TYPESCRIPT)
        .expect("the plugin's own settings are registered while it is enabled");
    handle.set("compiler_diagnostics", true).unwrap();

    let project = tempfile::tempdir().unwrap();
    let class_dir = project.path().join("src").join("classes").join("Door");
    std::fs::create_dir_all(&class_dir).unwrap();
    let class = class_dir.join("class.ts");
    std::fs::write(&class, ARROW).unwrap();

    let service = cx.new(LanguageServers::new);
    service.update(cx, |service, cx| {
        service.set_project(project.path().to_path_buf(), cx);
        service.set_check_debounce(Duration::from_millis(10));
    });
    let store = service.read_with(cx, |service, _| service.diagnostics());
    Setup { _project: project, class, service, store }
}

fn sources(cx: &mut TestAppContext, setup: &Setup) -> Vec<String> {
    setup
        .store
        .read_with(cx, |store, _| store.for_file(&setup.class))
        .into_iter()
        .map(|(source, _)| source.label())
        .collect()
}

#[gpui::test]
fn disabling_typescript_clears_both_sources_and_removes_the_server(cx: &mut TestAppContext) {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = setup(cx);

    // Opening a class routes to the plugin's server (which cannot be found: a
    // missing status, nothing spawned) ...
    let server = s
        .service
        .update(cx, |service, cx| service.server_for(&s.class, cx))
        .expect("the TypeScript plugin claims .ts files");
    let id = server.read_with(cx, |server, _| server.id().clone());
    assert_eq!(id.server, "typescript-language-server");
    assert_eq!(id.root, s.class.parent().unwrap().parent().unwrap().to_path_buf(), "rooted at the class folder");

    // ... which publishes something (put by hand: the process is not the
    // subject), while the compiler checks the unsaved text.
    s.store.update(cx, |store, cx| {
        store.set(DiagnosticSource::Server(id.clone()), &s.class, vec![diagnostic(&s.class, "from tsserver")], cx)
    });
    s.service.update(cx, |service, cx| service.check_source(&s.class, ARROW, cx));
    cx.executor().advance_clock(Duration::from_secs(1));
    wait_for(cx, "both sources", |cx| sources(cx, &s).len() == 2);
    let labels = sources(cx, &s);
    assert!(labels.contains(&"typescript-language-server".to_string()) && labels.contains(&"typescript compiler".to_string()));

    // The project turns the plugin off; the service notices on its tick.
    activate(&[TYPESCRIPT]);
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(sources(cx, &s).is_empty(), "both sources are gone: {:?}", sources(cx, &s));
    assert!(s.service.read_with(cx, |service, _| service.servers().is_empty()), "the server and its footer entry are gone");
    assert!(
        engine_state::global_config()
            .owner_handle(pulsar_settings::plugin::NS, SERVER_GROUP)
            .is_none(),
        "its settings group is withdrawn"
    );
    assert!(!s.service.read_with(cx, |service, _| service.has_compiler_for(&s.class)));
    assert!(s.service.update(cx, |service, cx| service.server_for(&s.class, cx)).is_none(), "no server for .ts");
    // An edit of the file now checks nothing.
    s.service.update(cx, |service, cx| service.check_source(&s.class, ARROW, cx));
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(sources(cx, &s).is_empty());

    pulsar_plugins::deactivate();
}

#[gpui::test]
fn the_plugins_own_setting_turns_the_compiler_source_off_and_on(cx: &mut TestAppContext) {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let s = setup(cx);
    s.service.update(cx, |service, cx| service.check_source(&s.class, ARROW, cx));
    cx.executor().advance_clock(Duration::from_secs(1));
    wait_for(cx, "the compiler's problem", |cx| sources(cx, &s) == ["typescript compiler"]);
    let entries = s.store.read_with(cx, |store, _| store.for_file(&s.class));
    assert_eq!(entries[0].1.source.as_deref(), Some("Pulsar"));

    let handle = engine_state::global_config()
        .owner_handle(pulsar_settings::plugin::NS, TYPESCRIPT)
        .unwrap();
    handle.set("compiler_diagnostics", false).unwrap();
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(sources(cx, &s).is_empty(), "off: the source is cleared");

    handle.set("compiler_diagnostics", true).unwrap();
    cx.executor().advance_clock(Duration::from_secs(5));
    wait_for(cx, "the check to resume", |cx| sources(cx, &s) == ["typescript compiler"]);

    pulsar_plugins::deactivate();
}
