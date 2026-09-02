//! End-to-end generated-project drift check (#652).
//!
//! Generates a COMPLETE minimal game project — engine bootstrap files from
//! `core_project_builder` plus a blueprint class from the vendored pbgc —
//! into a temp dir and runs `cargo check` on it against the exact crate pins
//! the running engine resolves. This is the full-fidelity guard behind the
//! fast in-tree probes in `pulsar_game::blueprint_codegen_drift`: it catches
//! anything the probe shape cannot (manifest baking, patch-table resolution,
//! `engine_main` bootstrap code, class-tree wiring).
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
//! in-repo paths — nothing machine-local).

use std::path::Path;
use std::process::Command;

/// Generate the full project and `cargo check` it against current pins.
#[test]
#[ignore = "heavy: cargo-checks a whole generated game project; run via `just ci-drift-check`"]
fn generated_project_compiles_against_current_pins() {
    let project = tempfile::tempdir().expect("temp project dir");

    // 1. Engine-owned bootstrap: Cargo.toml (baked deps + patches), main.rs,
    //    lib.rs (PIE shim), engine_main.rs, Pulsar/level.json.
    engine_backend::services::ensure_core_bootstrap(project.path())
        .expect("bootstrap files for the generated project");

    // 2. One blueprint class through the vendored generator's public pipeline —
    //    graph → compiled logic → actor file, the exact chain the Blueprint
    //    Editor runs. (#651: compiled logic functions receive the live-world
    //    slice, and the actor impl forwards its `(entity, world)` to them.)
    let mut graph = pbgc::GraphDescription::new("drift_sample");
    let mut begin =
        pbgc::NodeInstance::new("begin", "begin_play", pbgc::Position { x: 0.0, y: 0.0 });
    begin.outputs.push(pbgc::PinInstance::new(
        "begin_exec",
        pbgc::Pin::new(
            "begin_exec",
            "Body",
            pbgc::DataType::Exec,
            pbgc::PinType::Output,
        ),
    ));
    graph.add_node(begin);
    let logic = pbgc::compile_graph(&graph).expect("logic compilation");

    let spec = pbgc::ProjectSpec::new("drift_sample")
        .add_blueprint(pbgc::CompiledBlueprint::new("drift_probe", logic).with_begin_play(true));
    let generated = pbgc::generate_project(&spec);
    generated
        .write_to_dir(project.path())
        .expect("class tree written into the project");

    // 3. The bootstrap scans src/classes/ to regenerate classes/mod.rs; run
    //    it again now that the class directory exists so the module tree is
    //    fully wired.
    engine_backend::services::ensure_core_bootstrap(project.path())
        .expect("classes/mod.rs regeneration after writing the class tree");

    // 4. Verify generation actually produced a compilable crate before
    //    invoking cargo (guards against silent virtual-fs no-ops). The
    //    scripts/ crate (#653) is part of that contract: it must exist AND
    //    its path dependencies must resolve to real manifests (a wrong
    //    engine-checkout anchor otherwise surfaces only as cargo ENOENT).
    for required in [
        "Cargo.toml",
        "src/main.rs",
        "src/lib.rs",
        "src/classes/mod.rs",
        "src/classes/drift_probe/events/events.rs",
    ] {
        assert!(
            project.path().join(required).exists(),
            "generated project is missing {required}"
        );
    }
    let script_manifest = std::fs::read_dir(project.path().join("scripts"))
        .ok()
        .and_then(|entries| {
            entries
                .flatten()
                .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        })
        .map(|e| e.path().join("Cargo.toml"))
        .expect("scripts/ crate scaffolded");
    assert!(
        script_manifest.exists(),
        "scaffolded script crate manifest missing: {}",
        script_manifest.display()
    );
    let script_text =
        std::fs::read_to_string(&script_manifest).expect("script crate manifest readable");
    const PATH_KEY: &str = "path = \"";
    for line in script_text.lines() {
        let Some(start) = line.find(PATH_KEY) else {
            continue;
        };
        let rest = &line[start + PATH_KEY.len()..];
        let Some(end) = rest.find('"') else { continue };
        let dep_path = &rest[..end];
        let resolved = if Path::new(dep_path).is_absolute() {
            std::path::PathBuf::from(dep_path)
        } else {
            script_manifest.parent().unwrap().join(dep_path)
        };
        assert!(
            resolved.join("Cargo.toml").exists(),
            "script crate path dep does not resolve: {dep_path} -> {}",
            resolved.display()
        );
    }

    // 5. Compile it. A shared target dir makes repeat runs incremental.
    let started = std::time::Instant::now();
    let status =
        cargo_check(project.path()).unwrap_or_else(|e| panic!("failed to spawn cargo check: {e}"));
    println!(
        "cargo check of the generated project finished in {:?} ({status})",
        started.elapsed()
    );
    assert!(
        status.success(),
        "freshly generated project failed to compile against current pins \
         (signature/crate drift between vendored pbgc and pinned revs?)"
    );
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
        eprintln!(
            "cargo check stdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        eprintln!(
            "cargo check stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.status)
}

fn cargo_exe() -> std::path::PathBuf {
    std::env::var_os("CARGO")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("cargo"))
}
