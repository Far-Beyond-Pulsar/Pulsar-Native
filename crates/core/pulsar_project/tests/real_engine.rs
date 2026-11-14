//! Sync a project against this engine checkout and `cargo check` it: the
//! full-fidelity check that the mirror holds everything the game's graph
//! reads, that mirrored manifests stand alone, and that the generated game
//! crate (glue, a scripts crate, a class compiled to Rust) compiles against
//! the real engine. Heavy (the first run compiles the engine's game graph),
//! so ignored by default:
//!
//! ```text
//! cargo test -p pulsar_project --test real_engine -- --ignored --nocapture
//! ```

mod common;

use std::path::{Path, PathBuf};

use common::{cargo, write};
use pulsar_project::sdk::SdkKind;
use pulsar_project::{sync_with, EngineSdk, SyncOptions};
use pulsar_script_vm::{BinOp, Constant, Function, Instr, Module, Type, Variable};

fn engine_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// A class whose `tick` counts.
fn counter() -> Module {
    let mut m = Module::new("Door");
    m.variables = vec![Variable {
        name: "count".into(),
        ty: Type::Int,
        default: Some(Constant::Int(0)),
        id: None,
    }];
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
            Instr::Binary {
                op: BinOp::Add,
                dst: 1,
                a: 1,
                b: 2,
            },
            Instr::StoreVar { var: 0, src: 1 },
            Instr::Return { value: None },
        ],
        debug: None,
    });
    m
}

#[test]
#[ignore = "heavy: cargo-checks a generated project against the real engine"]
fn a_generated_project_compiles_against_this_engine() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("Real Check");
    write(&project.join("Pulsar.toml"), "[project]\nname = \"Real Check\"\n");
    write(
        &project.join("scripts/gameplay/Cargo.toml"),
        "[package]\nname = \"gameplay\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\npulsar_game = { workspace = true }\n",
    );
    write(
        &project.join("scripts/gameplay/src/lib.rs"),
        "pub fn register_scripts(_game: &mut pulsar_game::tick::TickLoop) -> Result<(), String> {\n    Ok(())\n}\n",
    );
    write(
        &project.join("Door.class/events/.build/module.json"),
        &counter().to_json().unwrap(),
    );
    write(
        &project.join("Door.class/graph_save.json"),
        "{\"compile_target\": \"rust\"}",
    );

    let sdk = EngineSdk::at(&engine_root()).unwrap();
    assert_eq!(sdk.kind(), SdkKind::Checkout);
    let started = std::time::Instant::now();
    let report = sync_with(&project, &sdk, SyncOptions::default()).expect("sync");
    println!(
        "sync: {:?}, {} SDK files written, problems: {:?}",
        started.elapsed(),
        report.sdk.written,
        report.problems
    );
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let mirror_bytes: u64 = common::tree(&project.join(".pulsar/sdk"))
        .iter()
        .map(|(_, bytes)| bytes.len() as u64)
        .sum();
    println!("mirror: {:.1} MiB", mirror_bytes as f64 / (1024.0 * 1024.0));

    let started = std::time::Instant::now();
    let report = sync_with(&project, &sdk, SyncOptions::default()).expect("second sync");
    println!("second sync: {:?}", started.elapsed());
    assert!(!report.changed(), "{report:?}");

    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("pulsar_project_real_check_target"));
    let output = cargo()
        .args(["check", "--workspace", "--manifest-path"])
        .arg(project.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", target)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let tail: Vec<&str> = stderr.lines().rev().take(120).collect();
        panic!(
            "the generated project does not compile:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("warning: unused manifest key") || l.contains("patch") && l.starts_with("warning"))
        .collect();
    println!("manifest warnings: {warnings:#?}");
}
