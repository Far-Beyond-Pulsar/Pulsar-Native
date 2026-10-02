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

/// Run a real PBGC-generated actor and record the current class-variable
/// behavior in both Cargo profiles. PBGC currently backs class variables with
/// thread-local statics; two actors of the same generated class therefore
/// share them on this thread. This is a baseline assertion, not a desired
/// long-term contract.
#[test]
#[ignore = "heavy: builds and runs a generated game project in debug and release"]
fn generated_rust_variable_baseline_runs_in_debug_and_release() {
    let project = tempfile::tempdir().expect("temp project dir");
    engine_backend::services::ensure_core_bootstrap(project.path())
        .expect("bootstrap generated project");

    let mut graph = pbgc::GraphDescription::new("shared_variable_probe");
    graph.add_node(event_node("begin", "begin_play", "begin_exec"));
    graph.add_node(event_node("tick", "tick", "tick_exec"));

    graph.add_node(setter_node("initialize", "count", 0.0));

    let mut getter = pbgc::NodeInstance::new(
        "read_count",
        "get_count",
        pbgc::Position { x: 100.0, y: 0.0 },
    );
    getter.outputs.push(pin(
        "read_count_result",
        "result",
        "i64",
        pbgc::PinType::Output,
    ));
    graph.add_node(getter);

    let mut add = pbgc::NodeInstance::new("increment", "add", pbgc::Position { x: 200.0, y: 0.0 });
    add.inputs
        .push(pin("increment_a", "a", "i64", pbgc::PinType::Input));
    add.inputs
        .push(pin("increment_b", "b", "i64", pbgc::PinType::Input));
    add.inputs[1].pin.data_type = pbgc::DataType::typed("i64");
    add.properties
        .insert("increment_b".into(), serde_json::json!(1.0));
    add.outputs.push(pin(
        "increment_result",
        "result",
        "i64",
        pbgc::PinType::Output,
    ));
    graph.add_node(add);

    graph.add_node(setter_node("assign", "count", 0.0));

    graph.add_connection(connection(
        "begin",
        "begin_exec",
        "initialize",
        "initialize_exec",
    ));
    graph.add_connection(connection("tick", "tick_exec", "assign", "assign_exec"));
    graph.add_connection(data_connection(
        "read_count",
        "read_count_result",
        "increment",
        "increment_a",
    ));
    graph.add_connection(data_connection(
        "increment",
        "increment_result",
        "assign",
        "assign_value",
    ));

    let source = pbgc::compile_graph_with_variables(
        &graph,
        std::collections::HashMap::from([("count".to_string(), "i64".to_string())]),
    )
    .expect("PBGC graph compilation");
    let blueprint = pbgc::CompiledBlueprint::new("shared_variable_probe", source)
        .with_begin_play(true)
        .with_tick(true)
        .with_variables(vec![pbgc::CompiledVariable {
            name: "count".into(),
            rust_type: "i64".into(),
            default_value: Some("0".into()),
        }]);
    let generated = pbgc::generate_project(
        &pbgc::ProjectSpec::new("generated_variable_baseline").add_blueprint(blueprint),
    );
    generated
        .write_to_dir(project.path())
        .expect("write PBGC output");
    engine_backend::services::ensure_core_bootstrap(project.path())
        .expect("wire generated class module");

    // SceneDB's manifest contains a historical double-slash Git URL for its
    // Reflection dependency. The generated project's ordinary patch table is
    // keyed by the normalized URL, so offline Cargo otherwise asks for a Git
    // revision that is not cached. Patch that temporary-project source to the
    // exact workspace crates already used to build this test.
    let reflection = local_path("../../../crates/third-party/pulsar-reflection/pulsar_reflection");
    let reflection_derive = local_path(
        "../../../crates/third-party/pulsar-reflection/pulsar_reflection_derive",
    );
    let generated_manifest = project.path().join("Cargo.toml");
    let mut manifest = std::fs::read_to_string(&generated_manifest)
        .expect("generated project manifest");
    manifest.push_str(&format!(
        "\n[patch.\"https://github.com//Far-Beyond-Pulsar/Pulsar-Reflection\"]\npulsar_reflection = {{ path = \"{reflection}\" }}\npulsar_reflection_derive = {{ path = \"{reflection_derive}\" }}\n"
    ));
    std::fs::write(&generated_manifest, manifest).expect("patch temporary manifest");

    // Add a read-only probe inside the generated vars module so the harness
    // can observe PBGC's private storage without changing its behavior.
    let vars_path = project
        .path()
        .join("src/classes/shared_variable_probe/vars/mod.rs");
    let vars = std::fs::read_to_string(&vars_path).expect("generated vars module");
    std::fs::write(
        &vars_path,
        format!("{vars}\npub fn baseline_count() -> Option<i64> {{ PBGC_VAR_COUNT.get() }}\n"),
    )
    .expect("instrument generated variable read");

    let bin_dir = project.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).expect("create generated probe binary dir");
    std::fs::write(
        bin_dir.join("generated_variable_baseline.rs"),
        r#"
#[path = "../classes/mod.rs"]
mod classes;
use pulsar_game::prelude::{Actor, Entity, World};
fn main() {
    let mut world = World::new();
    let mut first = classes::shared_variable_probe::SharedVariableProbe::default();
    let mut second = classes::shared_variable_probe::SharedVariableProbe::default();
    first.begin_play(Entity::DANGLING, &mut world);
    second.begin_play(Entity::DANGLING, &mut world);
    first.tick(Entity::DANGLING, &mut world);
    println!("after_first={:?}", classes::shared_variable_probe::vars::baseline_count());
    second.tick(Entity::DANGLING, &mut world);
    println!("after_second={:?}", classes::shared_variable_probe::vars::baseline_count());
    first.tick(Entity::DANGLING, &mut world);
    println!("after_first_again={:?}", classes::shared_variable_probe::vars::baseline_count());
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
        for expected in [
            "after_first=Some(1)",
            "after_second=Some(2)",
            "after_first_again=Some(3)",
        ] {
            assert!(
                stdout.contains(expected),
                "missing {expected} in {profile} output:\n{stdout}"
            );
        }
        println!("generated Rust {profile} variable trace:\n{stdout}");
    }
}

fn pin(id: &str, name: &str, type_name: &str, pin_type: pbgc::PinType) -> pbgc::PinInstance {
    pbgc::PinInstance::new(
        id,
        pbgc::Pin::new(id, name, pbgc::DataType::typed(type_name), pin_type),
    )
}

fn event_node(id: &str, node_type: &str, pin_id: &str) -> pbgc::NodeInstance {
    let mut node = pbgc::NodeInstance::new(id, node_type, pbgc::Position { x: 0.0, y: 0.0 });
    node.outputs.push(pbgc::PinInstance::new(
        pin_id,
        pbgc::Pin::new(pin_id, "Body", pbgc::DataType::Exec, pbgc::PinType::Output),
    ));
    node
}

fn setter_node(id: &str, variable: &str, value: f64) -> pbgc::NodeInstance {
    let mut node = pbgc::NodeInstance::new(
        id,
        &format!("set_{variable}"),
        pbgc::Position { x: 100.0, y: 0.0 },
    );
    node.inputs.push(pbgc::PinInstance::new(
        format!("{id}_exec"),
        pbgc::Pin::new(
            format!("{id}_exec"),
            "exec",
            pbgc::DataType::Exec,
            pbgc::PinType::Input,
        ),
    ));
    node.inputs.push(pin(
        &format!("{id}_value"),
        "value",
        "i64",
        pbgc::PinType::Input,
    ));
    node.properties
        .insert(format!("{id}_value"), serde_json::json!(value));
    node.outputs.push(pbgc::PinInstance::new(
        format!("{id}_next"),
        pbgc::Pin::new(
            format!("{id}_next"),
            "exec",
            pbgc::DataType::Exec,
            pbgc::PinType::Output,
        ),
    ));
    node
}

fn connection(from_node: &str, from_pin: &str, to_node: &str, to_pin: &str) -> pbgc::Connection {
    pbgc::Connection::new(
        from_node,
        from_pin,
        to_node,
        to_pin,
        pbgc::ConnectionType::Execution,
    )
}

fn data_connection(
    from_node: &str,
    from_pin: &str,
    to_node: &str,
    to_pin: &str,
) -> pbgc::Connection {
    pbgc::Connection::new(
        from_node,
        from_pin,
        to_node,
        to_pin,
        pbgc::ConnectionType::Data,
    )
}

fn cargo_run_generated_probe(
    project_dir: &Path,
    profile: &str,
) -> std::io::Result<std::process::Output> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("pulsar_drift_check_target"));
    let mut cmd = Command::new(cargo_exe());
    cmd.arg("run")
        .arg("--bin")
        .arg("generated_variable_baseline");
    if profile == "release" {
        cmd.arg("--release");
    }
    cmd.current_dir(project_dir)
        .env("CARGO_TARGET_DIR", target_dir)
        .output()
}

fn local_path(relative_to_game_crate: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(relative_to_game_crate)
        .canonicalize()
        .expect("workspace dependency path")
        .to_string_lossy()
        .replace('\\', "/")
}

fn cargo_exe() -> std::path::PathBuf {
    std::env::var_os("CARGO")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("cargo"))
}
