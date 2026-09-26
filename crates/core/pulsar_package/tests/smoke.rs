//! Packaging smoke test (#926, #889).
//!
//! Packages `tests/fixtures/smoke_project` (one level, a `Spawner` class
//! whose `begin_play` spawns a `Minion` and broadcasts `Spawner.Ping`,
//! which its own handler counts, plus a `GameRules` global script), copies
//! the output to a fresh directory, deletes the project copy it was built
//! from, and runs the packaged content headlessly for a few frames through
//! `pulsar_game::standalone::run_with`, the function the generated game's
//! `main()` calls. It then checks that `begin_play` ran, the spawn happened
//! and the event handler fired, from the pak and from loose files, and that
//! no shipped file names an absolute path or `CARGO_MANIFEST_DIR`.
//!
//! The game executable itself is not built here (`skip_build`): a release
//! build of a generated project compiles the whole engine. The binary runs
//! this same code path (`standalone::run`), so this is the packaged
//! runtime minus process start-up.

use std::path::{Path, PathBuf};

use pulsar_content::BuildProfile;
use pulsar_game::prelude::TickLoop;
use pulsar_game::standalone::{run_with, HeadlessReport, LaunchOptions};
use pulsar_script_vm::{
    BinOp, Constant, EventDecl, EventField, EventRef, Function, Import, Instr, Module, Param, Signature,
    Subscription, SubscriptionScope, Type, Variable,
};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/smoke_project")
}

// ---- the fixture's compiled modules ------------------------------------------
//
// Hand-assembled engine modules (the format every language compiles to), so
// the smoke test needs no scripting-language plugin. `fixture_modules_are_current`
// keeps the committed JSON in step with these; regenerate with
// `cargo test -p pulsar_package --test smoke -- --ignored regenerate_fixture_modules`.

fn function(name: &str, exported: bool, params: Vec<Type>, extra: Vec<Type>, code: Vec<Instr>) -> Function {
    let mut registers = params.clone();
    registers.extend(extra);
    Function { name: name.into(), exported, params, ret: Type::Unit, registers, code, debug: None }
}

fn import(name: &str, params: Vec<Type>, ret: Type) -> Import {
    Import { name: name.into(), sig: Signature::new(params.into_iter().map(Param::new), ret) }
}

/// `begin_play`: spawn a `Minion` at the origin into `minion`, then
/// broadcast `Spawner.Ping(1)`. `on_ping(amount)`: `pings += amount`.
fn spawner() -> Module {
    let mut m = Module::new("Spawner");
    m.variables = vec![
        Variable { name: "minion".into(), ty: Type::Entity, default: None },
        Variable { name: "pings".into(), ty: Type::Int, default: Some(Constant::Int(0)) },
    ];
    m.constants = vec![
        Constant::Str("Minion".into()),
        Constant::Float(0.0),
        Constant::Str("Spawner.Ping".into()),
        Constant::Int(1),
    ];
    m.imports = vec![
        import("world::spawn", vec![Type::Str, Type::Float, Type::Float, Type::Float], Type::Entity),
        import("event::emit", vec![Type::Str, Type::Int], Type::Unit),
    ];
    m.events = vec![EventDecl { name: "Spawner.Ping".into(), fields: vec![EventField::new("amount", Type::Int)] }];
    m.functions = vec![
        function("begin_play", true, vec![], vec![Type::Str, Type::Float, Type::Entity, Type::Str, Type::Int], vec![
            Instr::Const { dst: 0, index: 0 },
            Instr::Const { dst: 1, index: 1 },
            Instr::CallNative { import: 0, args: vec![0, 1, 1, 1], dst: Some(2) },
            Instr::StoreVar { var: 0, src: 2 },
            Instr::Const { dst: 3, index: 2 },
            Instr::Const { dst: 4, index: 3 },
            Instr::CallNative { import: 1, args: vec![3, 4], dst: None },
            Instr::Return { value: None },
        ]),
        function("on_ping", false, vec![Type::Int], vec![Type::Int], vec![
            Instr::LoadVar { dst: 1, var: 1 },
            Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 0 },
            Instr::StoreVar { var: 1, src: 1 },
            Instr::Return { value: None },
        ]),
    ];
    m.subscriptions = vec![Subscription {
        event: EventRef::Name("Spawner.Ping".into()),
        handler: 1,
        scope: SubscriptionScope::Global,
    }];
    m
}

/// `begin_play`: `alive = true`.
fn minion() -> Module {
    let mut m = Module::new("Minion");
    m.variables = vec![Variable { name: "alive".into(), ty: Type::Bool, default: None }];
    m.constants = vec![Constant::Bool(true)];
    m.functions = vec![function("begin_play", true, vec![], vec![Type::Bool], vec![
        Instr::Const { dst: 0, index: 0 },
        Instr::StoreVar { var: 0, src: 0 },
        Instr::Return { value: None },
    ])];
    m
}

/// Global script. `tick`: `ticks += 1`.
fn game_rules() -> Module {
    let mut m = Module::new("GameRules");
    m.variables = vec![Variable { name: "ticks".into(), ty: Type::Int, default: None }];
    m.constants = vec![Constant::Int(1)];
    m.functions = vec![function("tick", true, vec![Type::Float], vec![Type::Int, Type::Int], vec![
        Instr::LoadVar { dst: 1, var: 0 },
        Instr::Const { dst: 2, index: 0 },
        Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
        Instr::StoreVar { var: 0, src: 1 },
        Instr::Return { value: None },
    ])];
    m
}

fn fixture_modules() -> Vec<(&'static str, Module)> {
    vec![("Spawner", spawner()), ("Minion", minion()), ("GameRules", game_rules())]
}

fn module_path(root: &Path, class: &str) -> PathBuf {
    root.join("src/classes").join(class).join("events/.build/module.json")
}

#[test]
#[ignore = "writes the fixture's module.json files; run after changing the modules above"]
fn regenerate_fixture_modules() {
    for (class, module) in fixture_modules() {
        std::fs::write(module_path(&fixture(), class), module.to_json().unwrap() + "\n").unwrap();
    }
}

#[test]
fn fixture_modules_are_current() {
    for (class, module) in fixture_modules() {
        let committed = std::fs::read_to_string(module_path(&fixture(), class)).unwrap();
        assert_eq!(
            Module::from_json(&committed).unwrap(),
            module,
            "{class}: run the ignored `regenerate_fixture_modules` test"
        );
    }
}

// ---- helpers --------------------------------------------------------------------

/// The run's log, as a CI job reading the game's output would see it.
#[derive(Clone, Default)]
struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Log;
    fn make_writer(&'a self) -> Log {
        self.clone()
    }
}

impl Log {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// What the generated `engine_main::setup()` does for a project with no
/// native actors: turn on scripting for the installed content.
fn generated_setup(game: &mut TickLoop) -> Result<(), String> {
    game.enable_project_scripting()?;
    Ok(())
}

fn run_packaged(content: &Path, frames: u64) -> HeadlessReport {
    let options = LaunchOptions { headless: true, frames: Some(frames), content: Some(content.to_path_buf()) };
    let report = run_with(options, generated_setup).expect("headless run").expect("a headless report");
    eprintln!("{}{}", pulsar_game::standalone::HEADLESS_REPORT_PREFIX, report.to_json());
    report
}

fn int_var(report: &HeadlessReport, class: &str, var: &str) -> i64 {
    let instance = report.instance_of(class).unwrap_or_else(|| panic!("one {class} instance: {report:#?}"));
    instance.variables.get(var).and_then(|v| v.as_i64()).unwrap_or_else(|| panic!("{class}.{var}: {instance:?}"))
}

fn check_report(report: &HeadlessReport, pak: bool, frames: u64) {
    assert!(report.ok(), "script problems: {:#?}", report.problems);
    assert!(report.packaged);
    assert_eq!(report.pak, pak);
    assert_eq!(report.profile, "shipping");
    assert_eq!(report.level.as_deref(), Some("scenes/main.level"), "startup level from the cooked settings");
    assert_eq!(report.frames, frames);
    // begin_play ran on the placed Spawner and on the Minion it spawned.
    assert!(report.scripts.started >= 3, "Spawner, Minion and GameRules started: {:?}", report.scripts);
    assert_eq!(report.scripts.spawned, 1, "Spawner's begin_play spawned one Minion");
    let minion = report.instance_of("Minion").expect("the spawned Minion runs its script");
    assert_eq!(minion.variables.get("alive"), Some(&serde_json::json!(true)), "Minion begin_play ran");
    // The event handler fired once, for the one broadcast.
    assert_eq!(int_var(report, "Spawner", "pings"), 1, "Spawner.Ping handler ran");
    // The global script ticked every frame after its first.
    assert!(int_var(report, "GameRules", "ticks") >= (frames as i64) - 1);
    // Bound to the placed object, with its stable id.
    let spawner = report.instance_of("Spawner").unwrap();
    assert!(spawner.id.starts_with("spawner::"), "{}", spawner.id);
}

// ---- the smoke test ---------------------------------------------------------------

#[test]
fn packaged_game_runs_from_a_clean_directory() {
    let log = Log::default();
    let _ = tracing_subscriber::fmt().with_writer(log.clone()).with_ansi(false).try_init();
    let work = tempfile::tempdir().unwrap();
    // Package a copy, so class-id or slot-id bookkeeping never touches the
    // source tree.
    let project = work.path().join("project");
    copy_dir(&fixture(), &project);

    let mut options = pulsar_package::PackageOptions::new(&project, work.path().join("out-pak"));
    options.profile = BuildProfile::Shipping;
    options.skip_build = true;
    let report = pulsar_package::package(&options).expect("package (pak)");
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    assert_eq!(report.levels, ["scenes/main.level"]);
    assert_eq!(report.startup_level.as_deref(), Some("scenes/main.level"));
    assert_eq!(report.assets, 1);
    assert!(report.files.contains_key("assets/meshes/glass_red.mesh"));
    assert!(report.files.contains_key("src/classes/Spawner/events/.build/module.pvm"));
    assert!(!report.files.keys().any(|f| f.ends_with("module.json")), "shipped modules are binary");

    let mut loose = options.clone();
    loose.out = work.path().join("out-loose");
    loose.loose = true;
    pulsar_package::package(&loose).expect("package (loose)");

    // Copy the packages somewhere else and remove the project: the games
    // must not need it.
    let shipped = tempfile::tempdir().unwrap();
    copy_dir(&work.path().join("out-pak"), &shipped.path().join("pak"));
    copy_dir(&work.path().join("out-loose"), &shipped.path().join("loose"));
    std::fs::remove_dir_all(&project).unwrap();
    drop(work);

    // Shipped data: no machine paths, a registry for the asset, cooked levels.
    let pak_content = shipped.path().join("pak/Content");
    let pak = pulsar_content::PakReader::open(pak_content.join("game.pak")).unwrap();
    for path in pak.entries().keys() {
        let bytes = pak.read(path).unwrap();
        if let Some(problem) = pulsar_package::machine_path_in(path, &bytes, &project) {
            panic!("{problem}");
        }
        assert!(!String::from_utf8_lossy(&bytes).contains("CARGO_MANIFEST_DIR"));
    }
    let registry = pulsar_content::AssetRegistry::from_json(&pak.read("Pulsar/asset_registry.json").unwrap()).unwrap();
    let mesh = registry.get("assets/meshes/glass_red.mesh").expect("registered asset");
    let location = mesh.pak.expect("packed");
    assert_eq!(pak.entry("assets/meshes/glass_red.mesh").map(|e| (e.offset, e.len)), Some((location.offset, location.len)));
    let level: serde_json::Value = serde_json::from_slice(&pak.read("scenes/main.level").unwrap()).unwrap();
    assert!(level.get("editor").is_none(), "editor camera dropped");
    assert!(level["objects"][0].get("locked").is_none(), "lock flag dropped");
    assert_eq!(
        level["objects"][0]["component_instances"][0]["data"]["class"],
        "762f7d93-1e8c-44dd-b20d-0707e5506e5f",
        "class GUID resolved"
    );
    assert_eq!(level["objects"][1]["props"]["mesh_asset"], "assets/meshes/glass_red.mesh");
    let scripting: serde_json::Value = serde_json::from_slice(&pak.read("Pulsar/scripting.json").unwrap()).unwrap();
    assert_eq!(scripting["global_scripts"][0], "7234fdfc-d1c3-4404-bf65-2f434d11b654");

    // The packaged game finds its content next to its executable.
    let found = pulsar_content::ContentRoot::discover_from(&shipped.path().join("pak"), None).unwrap();
    assert!(found.is_packaged() && found.pak().is_some());

    // Run it: from the pak, then from loose files.
    std::env::set_var("PULSAR_ASSET_CACHE", shipped.path().join("asset-cache"));
    let frames = 10;
    let report = run_packaged(&pak_content, frames);
    check_report(&report, true, frames);
    // Pak assets were unpacked for Helio's file-based mesh loader, and the
    // level's and the spawned Minion's meshes loaded from there.
    let cache = shipped.path().join("asset-cache");
    assert!(std::fs::read_dir(&cache).unwrap().next().is_some());
    let text = log.text();
    let loaded: Vec<&str> = text.lines().filter(|l| l.contains("StaticMeshComponent: loaded")).collect();
    assert!(loaded.len() >= 2, "floor and minion meshes loaded:\n{text}");
    assert!(loaded.iter().all(|l| l.contains("assets/meshes/glass_red.mesh") && l.contains("asset-cache")), "{loaded:?}");
    assert!(!text.contains("failed to load mesh"), "{text}");

    let report = run_packaged(&shipped.path().join("loose/Content"), frames);
    check_report(&report, false, frames);
}
