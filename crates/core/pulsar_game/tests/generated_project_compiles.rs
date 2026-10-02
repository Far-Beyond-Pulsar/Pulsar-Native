//! End-to-end generated-project drift check (#652).
//!
//! Generates a COMPLETE minimal game project (engine bootstrap files from
//! `core_project_builder` plus a class exported by `pulsar_script_codegen`)
//! into a temp dir and runs `cargo check` on it against the exact crate pins
//! the running engine resolves. This is the full-fidelity guard behind the
//! always-compiled probe in `pulsar_game::export_probe` (built from
//! `build.rs`): it catches anything that probe cannot (manifest baking,
//! patch-table resolution, `engine_main` bootstrap code, class-tree wiring).
//!
//! It is `#[ignore]`d by default because the first run compiles the entire
//! engine dependency tree for the generated project (subsequent runs reuse
//! the shared target dir). Run it explicitly:
//!
//! ```text
//! just ci-drift-check
//! ```
//!
//! The generated manifest bakes the engine workspace's own dependency specs
//! and `[patch]` tables at build time, so this check always exercises the
//! exact revs CI would resolve (all patched sources are git pins or
//! in-repo paths: nothing machine-local).

use std::path::Path;
use std::process::Command;

use pulsar_script_codegen::actor::class_files;
use pulsar_script_vm::{BinOp, Constant, Function, Instr, Module, Type, Variable};

/// A class with one variable, `count`, that `tick` increments.
fn counter_class(name: &str) -> Module {
    let mut m = Module::new(name);
    m.variables = vec![Variable { name: "count".into(), ty: Type::Int, default: Some(Constant::Int(0)), id: None }];
    m.constants = vec![Constant::Int(1)];
    m.functions.push(Function {
        name: "tick".into(),
        exported: true,
        params: vec![Type::Float],
        ret: Type::Unit,
        registers: vec![Type::Float, Type::Int, Type::Int],
        code: vec![
            Instr::LoadVar { dst: 1, var: 0 },
            Instr::Const { dst: 2, index: 0 },
            Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
            Instr::StoreVar { var: 0, src: 1 },
            Instr::Return { value: None },
        ],
        debug: None,
    });
    m
}

/// Write the engine bootstrap and one exported class into `project`.
fn generate_project(project: &Path, class: &str) {
    // 1. Engine-owned bootstrap: Cargo.toml (baked deps + patches), main.rs,
    //    lib.rs (PIE shim), engine_main.rs, Pulsar/level.json.
    engine_backend::services::ensure_core_bootstrap(project).expect("bootstrap files for the generated project");

    // 2. The class tree, through the exporter's public API: the same call the
    //    Blueprint Editor makes after compiling a class.
    for (path, content) in class_files(class, &counter_class(class), &[]).expect("the class exports") {
        let path = project.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).expect("class directory");
        std::fs::write(path, content).expect("class file");
    }

    // 3. The bootstrap scans src/classes/ to regenerate classes/mod.rs; run
    //    it again now that the class directory exists so the module tree is
    //    fully wired.
    engine_backend::services::ensure_core_bootstrap(project).expect("classes/mod.rs regeneration after writing the class tree");
}

/// Generate the full project and `cargo check` it against current pins.
#[test]
#[ignore = "heavy: cargo-checks a whole generated game project; run via `just ci-drift-check`"]
fn generated_project_compiles_against_current_pins() {
    let project = tempfile::tempdir().expect("temp project dir");
    generate_project(project.path(), "drift_probe");

    // Verify generation actually produced a compilable crate before invoking
    // cargo (guards against silent virtual-fs no-ops). The scripts/ crate
    // (#653) is part of that contract: it must exist AND its path
    // dependencies must resolve to real manifests (a wrong engine-checkout
    // anchor otherwise surfaces only as cargo ENOENT).
    for required in ["Cargo.toml", "src/main.rs", "src/lib.rs", "src/classes/mod.rs", "src/classes/drift_probe/events/events.rs"] {
        assert!(project.path().join(required).exists(), "generated project is missing {required}");
    }
    let script_manifest = std::fs::read_dir(project.path().join("scripts"))
        .ok()
        .and_then(|entries| entries.flatten().find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
        .map(|e| e.path().join("Cargo.toml"))
        .expect("scripts/ crate scaffolded");
    assert!(script_manifest.exists(), "scaffolded script crate manifest missing: {}", script_manifest.display());
    let script_text = std::fs::read_to_string(&script_manifest).expect("script crate manifest readable");
    const PATH_KEY: &str = "path = \"";
    for line in script_text.lines() {
        let Some(start) = line.find(PATH_KEY) else { continue };
        let rest = &line[start + PATH_KEY.len()..];
        let Some(end) = rest.find('"') else { continue };
        let dep_path = &rest[..end];
        let resolved = if Path::new(dep_path).is_absolute() {
            std::path::PathBuf::from(dep_path)
        } else {
            script_manifest.parent().unwrap().join(dep_path)
        };
        assert!(resolved.join("Cargo.toml").exists(), "script crate path dep does not resolve: {dep_path} -> {}", resolved.display());
    }

    // Compile it. A shared target dir makes repeat runs incremental.
    let started = std::time::Instant::now();
    let status = cargo_check(project.path()).unwrap_or_else(|e| panic!("failed to spawn cargo check: {e}"));
    println!("cargo check of the generated project finished in {:?} ({status})", started.elapsed());
    assert!(
        status.success(),
        "freshly generated project failed to compile against current pins \
         (signature/crate drift between the exporter and the pinned revs?)"
    );
}

/// Run an exported class in a real generated project, in both Cargo
/// profiles. Class variables belong to each actor: two actors of one class
/// keep separate counters, in debug and in release (the old Rust export
/// shared them through thread-local statics).
#[test]
#[ignore = "heavy: builds and runs a generated game project in debug and release"]
fn exported_class_variables_are_per_actor_in_debug_and_release() {
    let project = tempfile::tempdir().expect("temp project dir");
    generate_project(project.path(), "counter");

    // SceneDB's manifest contains a historical double-slash Git URL for its
    // Reflection dependency. The generated project's ordinary patch table is
    // keyed by the normalized URL, so offline Cargo otherwise asks for a Git
    // revision that is not cached. Patch that temporary-project source to the
    // same Pulsar-Reflection rev the workspace pins.
    let rev = "7ffd1932970310681e82d204c8f7a59eb7d67247";
    let generated_manifest = project.path().join("Cargo.toml");
    let mut manifest = std::fs::read_to_string(&generated_manifest).expect("generated project manifest");
    manifest.push_str(&format!(
        "\n[patch.\"https://github.com//Far-Beyond-Pulsar/Pulsar-Reflection\"]\npulsar_reflection = {{ git = \"https://github.com/Far-Beyond-Pulsar/Pulsar-Reflection\", rev = \"{rev}\" }}\npulsar_reflection_derive = {{ git = \"https://github.com/Far-Beyond-Pulsar/Pulsar-Reflection\", rev = \"{rev}\" }}\n"
    ));
    std::fs::write(&generated_manifest, manifest).expect("patch temporary manifest");

    let bin_dir = project.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).expect("create generated probe binary dir");
    std::fs::write(
        bin_dir.join("exported_counter.rs"),
        r#"
#[path = "../classes/mod.rs"]
mod classes;
use pulsar_game::prelude::{Actor, Entity, World};
fn count(actor: &classes::counter::Counter) -> String {
    format!("{:?}", actor.variable("count"))
}
fn main() {
    let mut world = World::new();
    let mut first = classes::counter::Counter::default();
    let mut second = classes::counter::Counter::default();
    first.begin_play(Entity::DANGLING, &mut world);
    second.begin_play(Entity::DANGLING, &mut world);
    first.tick(Entity::DANGLING, &mut world);
    println!("after_first={}", count(&first));
    second.tick(Entity::DANGLING, &mut world);
    println!("second={}", count(&second));
    first.tick(Entity::DANGLING, &mut world);
    println!("after_first_again={}", count(&first));
    println!("second_again={}", count(&second));
}
"#,
    )
    .expect("write generated runner");

    for profile in ["debug", "release"] {
        let output = cargo_run_generated_probe(project.path(), profile)
            .unwrap_or_else(|e| panic!("failed to launch generated {profile} probe: {e}"));
        assert!(
            output.status.success(),
            "generated {profile} probe failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        for expected in ["after_first=Some(1)", "second=Some(1)", "after_first_again=Some(2)", "second_again=Some(1)"] {
            assert!(stdout.contains(expected), "missing {expected} in {profile} output:\n{stdout}");
        }
        println!("exported {profile} variable trace:\n{stdout}");
    }
}

/// Run `cargo check` inside `project_dir`, reusing one shared target dir so
/// repeated drift checks are incremental rather than cold builds.
fn cargo_check(project_dir: &Path) -> std::io::Result<std::process::ExitStatus> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("pulsar_drift_check_target"));

    let mut cmd = Command::new(cargo_exe());
    cmd.arg("check")
        .current_dir(project_dir)
        .env("CARGO_TARGET_DIR", target_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Surface compiler errors on failure instead of swallowing them.
    let output = cmd.output()?;
    if !output.status.success() {
        eprintln!("cargo check stdout:\n{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("cargo check stderr:\n{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(output.status)
}

fn cargo_run_generated_probe(project_dir: &Path, profile: &str) -> std::io::Result<std::process::Output> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("pulsar_drift_check_target"));
    let mut cmd = Command::new(cargo_exe());
    cmd.arg("run").arg("--bin").arg("exported_counter");
    if profile == "release" {
        cmd.arg("--release");
    }
    cmd.current_dir(project_dir).env("CARGO_TARGET_DIR", target_dir).output()
}

fn cargo_exe() -> std::path::PathBuf {
    std::env::var_os("CARGO")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("cargo"))
}
