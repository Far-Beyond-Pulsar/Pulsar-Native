//! A language is one plugin registration (`pulsar_language::Language`), and the
//! host only routes (P3 of `.agents/LANGUAGE_PLUGINS_PLAN.md`):
//!
//! - a class with two languages' marker files is one `MultipleLanguages`
//!   problem, raised by the host at scan; neither plugin names the other;
//! - a third language (here a dummy with a marker and a server) needs no
//!   change in either plugin, and neither compiles its classes;
//! - disabling a plugin removes its language, its class markers and its
//!   server from what the host routes through;
//! - Rust is the `pulsar.rust` plugin (P4): enabled by default, it registers
//!   the Rust language and rust-analyzer's spec; disabled, a `.rs` file has no
//!   server, the server's diagnostics are cleared, its status entry and its
//!   settings group are gone.
//!
//! These tests need the plugins, so they are here (a kernel crate's tests
//! never link one: `architecture/layers.toml`).

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use pulsar_class::{ClassProblem, ClassRegistry};
use pulsar_language::{
    Language, LanguageRegistry, LanguageServerSpec, LocateError, ReadinessRule, RootStrategy,
    ServerHooks,
};
use serde_json::Value;

use installed_plugins as _;

const TYPESCRIPT: &str = "pulsar.typescript";
const RUST: &str = "pulsar.rust";
const BLUEPRINTS: &str = "pulsar.blueprints";

// A third language, owned by nobody here: it has only a marker. The tests
// below leave both plugins as they are.
pulsar_language::submit_test_language!("dummy", ["dummy.src"]);

/// A language whose plugin is TypeScript's (its origin is that plugin's root),
/// with a server: disabling the plugin must take the server away too.
static DUMMY_SERVER: LanguageServerSpec = LanguageServerSpec {
    id: "dummy-server",
    display_name: "Dummy server",
    find: |_| None,
    locate: |_| Err(LocateError::new("not used")),
    roots: RootStrategy::ProjectRoot,
    initialization_options: |_| Value::Null,
    configuration: |_, _| Value::Null,
    client_capabilities: || Value::Null,
    hooks: ServerHooks::DEFAULT,
    readiness: ReadinessRule::Immediate,
    max_indexing: Duration::from_secs(1),
    eager: false,
    commands: &[],
    prepare_workspace: None,
    installer: None,
};

const SERVED: Language = Language {
    origin: "plugin_typescript",
    id: "served",
    display_name: "Served",
    file_extensions: &["srv"],
    language_ids: &[],
    highlight_id: "",
    class_markers: &[],
    config_files: &[],
    compiler: None,
    server: Some(&DUMMY_SERVER),
};

pulsar_plugins::inventory::submit! { SERVED }

/// The tests change the process's activated plugins: one at a time.
static ACTIVATION: Mutex<()> = Mutex::new(());

fn enabled_but(off: &[&str]) {
    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default().with(off, false),
    ));
}

fn class(root: &Path, name: &str, files: &[&str]) -> std::path::PathBuf {
    let dir = root.join("src").join("classes").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    for file in files {
        std::fs::write(dir.join(file), "{}").unwrap();
    }
    dir
}

#[test]
fn both_plugins_register_one_language_each() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let registry = LanguageRegistry::current();
    let blueprint = registry.by_id("blueprint").expect("the Blueprint plugin's language");
    let typescript = registry.by_id("typescript").expect("the TypeScript plugin's language");
    assert_eq!(blueprint.class_markers, ["graph_save.json"]);
    assert_eq!(typescript.class_markers, ["class.ts"]);
    assert!(blueprint.compiler.is_some() && typescript.compiler.is_some());
    assert_eq!(typescript.file_extensions, ["ts", "mts", "cts"]);
    assert_eq!(
        pulsar_plugins::owner_of(blueprint.origin),
        pulsar_plugins::Owner::Plugin(pulsar_plugins::descriptor(BLUEPRINTS).unwrap())
    );
}

#[test]
fn a_class_with_two_markers_is_one_multiple_languages_problem() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("Pulsar")).unwrap();
    class(root.path(), "Both", &["graph_save.json", "class.ts"]);
    class(root.path(), "Graph", &["graph_save.json"]);
    class(root.path(), "Text", &["class.ts"]);

    let registry = ClassRegistry::scan(root.path());
    assert_eq!(registry.entries().len(), 3, "all three are classes");
    let problems = registry.problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    match &problems[0] {
        ClassProblem::MultipleLanguages { class, languages, .. } => {
            assert_eq!(class, "Both");
            assert_eq!(languages, &["blueprint", "typescript"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(pulsar_class::class_problems(root.path()), problems);
}

#[test]
fn neither_plugin_compiles_a_class_that_is_two_languages() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("Pulsar")).unwrap();
    let both = class(root.path(), "Both", &["graph_save.json", "class.ts"]);
    std::fs::write(both.join("class.ts"), "export default class Both extends ScriptClass {}").unwrap();
    let natives = pulsar_script_vm::NativeRegistry::with_engine_natives();
    for (language, compiler) in LanguageRegistry::current().compilers() {
        let problems = compiler.compile_project(root.path(), &natives);
        assert!(
            problems.is_empty(),
            "{}: the host hands a language only the classes it owns: {problems:?}",
            language.id
        );
    }
    let entry = ClassRegistry::scan(root.path()).by_name("Both").cloned().unwrap();
    assert!(
        !pulsar_class::ModuleStore::of_project(root.path()).has_module(&entry.id),
        "nothing was compiled"
    );
}

#[test]
fn a_third_language_needs_no_change_in_either_plugin() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("Pulsar")).unwrap();
    let alone = class(root.path(), "Alone", &["dummy.src"]);
    let with_ts = class(root.path(), "WithText", &["dummy.src", "class.ts"]);
    let with_graph = class(root.path(), "WithGraph", &["dummy.src", "graph_save.json"]);

    // The dummy's class is a class, and it is the dummy's alone.
    assert!(pulsar_class::registry::is_class_dir(&alone));
    assert!(pulsar_class::is_owned_by(&alone, "dummy"));
    assert!(!pulsar_class::is_owned_by(&alone, "blueprint"));
    assert!(!pulsar_class::is_owned_by(&alone, "typescript"));

    // It conflicts with either existing language, as they conflict with each
    // other, though neither plugin has heard of it.
    for (dir, other) in [(&with_ts, "typescript"), (&with_graph, "blueprint")] {
        match pulsar_class::language_of(dir) {
            Err(ClassProblem::MultipleLanguages { languages, .. }) => {
                assert_eq!(languages.len(), 2);
                assert!(languages.iter().any(|l| l == "dummy"), "{languages:?}");
                assert!(languages.iter().any(|l| l == other), "{languages:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    // The two plugins compile their own classes and leave the dummy's alone.
    let natives = pulsar_script_vm::NativeRegistry::with_engine_natives();
    for (_, compiler) in LanguageRegistry::current().compilers() {
        assert!(compiler.compile_project(root.path(), &natives).is_empty());
    }
}

#[test]
fn disabling_a_plugin_removes_its_markers_language_and_server() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("Pulsar")).unwrap();
    let text = class(root.path(), "Text", &["class.ts"]);
    let graph = class(root.path(), "Graph", &["graph_save.json"]);
    // Not a class of any language but TypeScript's: no `class.json` or prefab.
    assert!(pulsar_class::registry::is_class_dir(&text));
    assert!(pulsar_class::registry::is_class_dir(&graph));
    let served = Path::new("/p/a.srv");
    assert!(LanguageRegistry::current().server_for_path(served).is_some());
    let before = pulsar_plugins::generation();

    enabled_but(&[TYPESCRIPT]);
    assert_ne!(pulsar_plugins::generation(), before, "the registry follows the generation");
    let registry = LanguageRegistry::current();
    assert!(registry.by_id("typescript").is_none());
    assert!(registry.for_path(Path::new("src/a.ts")).is_none());
    assert!(
        !pulsar_class::registry::is_class_dir(&text),
        "its marker no longer makes a class"
    );
    assert!(pulsar_class::registry::is_class_dir(&graph), "the other language's does");
    assert!(
        registry.server_for_path(served).is_none(),
        "its server is no longer routed to"
    );
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(other.path().join("Pulsar")).unwrap();
    class(other.path(), "Text", &["class.ts"]);
    assert!(ClassRegistry::scan(other.path()).by_name("Text").is_none());

    enabled_but(&[BLUEPRINTS]);
    let registry = LanguageRegistry::current();
    assert!(registry.by_id("blueprint").is_none());
    assert!(!pulsar_class::registry::is_class_dir(&graph));
    assert!(pulsar_class::registry::is_class_dir(&text));
    assert!(registry.server_for_path(served).is_some());

    pulsar_plugins::deactivate();
    assert!(pulsar_class::registry::is_class_dir(&text));
    assert!(pulsar_class::registry::is_class_dir(&graph));
    assert!(LanguageRegistry::current().server_for_path(served).is_some());
}

#[test]
fn a_disabled_plugins_server_settings_group_is_withdrawn() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    // The standard group of a language's server is derived from its
    // registration and follows its plugin.
    let cfg = pulsar_config::ConfigManager::new();
    let groups = || {
        pulsar_plugins::all_registered::<pulsar_settings::plugin::SettingsGroups>()
            .flat_map(|source| (source.groups)())
            .collect::<Vec<_>>()
    };
    let report = pulsar_settings::plugin::sync_groups(&cfg, groups(), pulsar_plugins::origin_enabled);
    assert!(
        report.registered.iter().any(|owner| owner.ends_with("/dummy-server")),
        "{report:?}"
    );
    assert!(
        report.registered.iter().any(|owner| owner.ends_with("/rust-analyzer")),
        "{report:?}"
    );

    enabled_but(&[TYPESCRIPT]);
    let report = pulsar_settings::plugin::sync_groups(&cfg, groups(), pulsar_plugins::origin_enabled);
    assert!(
        report.withdrawn.iter().any(|owner| owner.ends_with("/dummy-server")),
        "{report:?}"
    );
    assert!(
        !report.withdrawn.iter().any(|owner| owner.ends_with("/rust-analyzer")),
        "another plugin's group stays: {report:?}"
    );

    // Disabling the Rust plugin withdraws its group, which lives under the
    // plugin's own page.
    enabled_but(&[RUST]);
    let report = pulsar_settings::plugin::sync_groups(&cfg, groups(), pulsar_plugins::origin_enabled);
    assert_eq!(report.withdrawn, ["pulsar.rust/rust-analyzer"], "{report:?}");
    pulsar_plugins::deactivate();
    let report = pulsar_settings::plugin::sync_groups(&cfg, groups(), pulsar_plugins::origin_enabled);
    assert!(report.registered.contains(&"pulsar.rust/rust-analyzer".to_string()), "{report:?}");
}

#[test]
fn rust_is_a_default_plugin_that_registers_the_rust_language() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let plugin = pulsar_plugins::descriptor(RUST).expect("pulsar.rust is installed");
    assert!(plugin.enabled_by_default && plugin.editor_only);
    assert_eq!(plugin.root, "rust_plugin");

    // Enabled with a project that lists no plugins at all: by default.
    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default(),
    ));
    let registry = LanguageRegistry::current();
    let rust = registry.by_id("rust").expect("the Rust plugin's language");
    assert_eq!(registry.for_path(Path::new("src/main.rs")).map(|l| l.id), Some("rust"));
    assert_eq!(rust.file_extensions, ["rs"]);
    assert_eq!(rust.config_files, ["Cargo.toml", "rust-project.json"]);
    assert!(rust.compiler.is_none() && rust.class_markers.is_empty());
    assert_eq!(
        pulsar_plugins::owner_of(rust.origin),
        pulsar_plugins::Owner::Plugin(plugin)
    );
    let (_, spec) = registry.server_for_path(Path::new("/p/src/main.rs")).expect("a server for .rs");
    assert_eq!(spec.id, "rust-analyzer");
    assert!(spec.eager && spec.readiness == ReadinessRule::Signal);
    assert!(pulsar_lsp::active_servers().iter().any(|(_, spec)| spec.id == "rust-analyzer"));
    // The old Rust settings keys follow the group to the plugin's page.
    let moves: Vec<_> = pulsar_plugins::registered::<pulsar_settings::plugin::SettingsMove>().collect();
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].from, "engine.pulsar_lsp/rust-analyzer");
    assert_eq!(
        pulsar_settings::plugin::owner_of(moves[0].origin, moves[0].id),
        "pulsar.rust/rust-analyzer"
    );
    pulsar_plugins::deactivate();
}

#[test]
fn with_rust_disabled_a_rs_file_has_no_server_and_no_settings_group() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    enabled_but(&[RUST]);
    let registry = LanguageRegistry::current();
    assert!(registry.by_id("rust").is_none());
    assert!(registry.for_path(Path::new("src/main.rs")).is_none());
    assert!(registry.server_for_path(Path::new("/p/src/main.rs")).is_none());
    assert!(!pulsar_lsp::active_servers().iter().any(|(_, spec)| spec.id == "rust-analyzer"));
    assert_eq!(
        pulsar_plugins::registered::<pulsar_settings::plugin::SettingsMove>().count(),
        0,
        "a disabled plugin's settings are left where they are"
    );
    // Other plugins' languages are untouched.
    assert!(registry.by_id("typescript").is_some());
    pulsar_plugins::deactivate();
}

/// The whole of "disabled": a running Rust server, with diagnostics of its
/// own and of another source, loses its plugin. The service stops and drops
/// it (its status entry is gone with it), the server's diagnostics are cleared
/// and every file that had one is announced so editors and the Problems drawer
/// clear, the other source's entry stays, the settings group is withdrawn.
/// Nothing is installed or spawned: the server is configured with a path that
/// does not exist, so locating it fails.
#[gpui::test]
fn disabling_rust_stops_its_server_and_clears_its_diagnostics(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    use pulsar_lsp::{DiagnosticSource, DiagnosticsEvent, LanguageServers, LanguageServersEvent};

    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    static BOUND: std::sync::Once = std::sync::Once::new();
    BOUND.call_once(|| pulsar_settings::register_all_settings(engine_state::global_config()));
    pulsar_settings::plugin::sync(engine_state::global_config());
    engine_state::global_config()
        .owner_handle(pulsar_settings::plugin::NS, "pulsar.rust/rust-analyzer")
        .expect("the Rust server group is registered while the plugin is enabled")
        .set("path", "/pulsar-test/no-such-rust-analyzer")
        .unwrap();

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
    let file = project.path().join("src").join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let service = cx.new(LanguageServers::new);
    service.update(cx, |service, cx| service.set_project(project.path().to_path_buf(), cx));
    let server = service.read_with(cx, |service, cx| {
        assert_eq!(service.servers().len(), 1, "the eager Rust server exists for a Cargo project");
        service.existing_server_for(&file, cx).expect("it handles .rs files")
    });
    let id = server.read_with(cx, |server, _| server.id().clone());
    assert_eq!(id.server, "rust-analyzer");

    let store = service.read_with(cx, |service, _| service.diagnostics());
    let diagnostic = |message: &str| ui::diagnostics::Diagnostic {
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
    };
    store.update(cx, |store, cx| {
        store.set(DiagnosticSource::Server(id.clone()), &file, vec![diagnostic("from the server")], cx);
        store.set(DiagnosticSource::Compiler("other"), &file, vec![diagnostic("from a compiler")], cx);
    });
    assert_eq!(store.read_with(cx, |store, _| store.for_file(&file).len()), 2);

    let diagnostics_events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let service_events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (seen_diagnostics, seen_service) = (diagnostics_events.clone(), service_events.clone());
    let _subscriptions = cx.update(|cx| {
        (
            cx.subscribe(&store, move |_, event: &DiagnosticsEvent, _| seen_diagnostics.borrow_mut().push(event.clone())),
            cx.subscribe(&service, move |_, event: &LanguageServersEvent, _| seen_service.borrow_mut().push(event.clone())),
        )
    });

    // The project turns the Rust plugin off; the service notices on its tick.
    enabled_but(&[RUST]);
    pulsar_settings::plugin::sync(engine_state::global_config());
    assert!(
        engine_state::global_config()
            .owner_handle(pulsar_settings::plugin::NS, "pulsar.rust/rust-analyzer")
            .is_none(),
        "the settings group is withdrawn"
    );
    cx.executor().advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();

    assert!(service.read_with(cx, |service, _| service.servers().is_empty()), "no status entry is left");
    assert!(
        service_events.borrow().iter().any(|event| *event == LanguageServersEvent::Removed(id.clone())),
        "{:?}",
        service_events.borrow()
    );
    let left = store.read_with(cx, |store, _| store.for_file(&file));
    assert_eq!(left.len(), 1, "only the other source's entry remains: {}", left.len());
    assert!(matches!(left[0].0, DiagnosticSource::Compiler("other")));
    assert!(!diagnostics_events.borrow().is_empty(), "the file is announced so views clear");
    service.update(cx, |service, cx| {
        assert!(service.server_for(&file, cx).is_none(), "no server claims a .rs file now");
        assert!(service.servers().is_empty());
    });

    pulsar_plugins::deactivate();
    pulsar_settings::plugin::sync(engine_state::global_config());
}
