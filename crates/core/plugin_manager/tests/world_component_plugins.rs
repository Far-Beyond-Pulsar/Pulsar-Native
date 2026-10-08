//! Plugin component classes as World components (Pulsar-Native#1083).
//!
//! A plugin library links its own static copy of the engine's world crates,
//! so its classes register into registries the editor never reads and its
//! component ids come from its own table. Attached to the editor's world
//! runtime at load, its copy shares the editor's process-wide world state
//! (component ids, counters, registries), as every copy of gpui shares
//! gpui's runtime, and its classes are live components of the editor's
//! `World`.
//!
//! The host and the plugin are fixtures (`tests/fixtures/world_plugins`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// `cargo build` the fixture packages into `target`; returns `target/debug`.
fn build(target: &str, packages: &[&str], features: &[&str]) -> PathBuf {
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(target);
    let mut command = Command::new(cargo());
    command.args(["build", "--quiet"]);
    for package in packages {
        command.args(["-p", package]);
    }
    if !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }
    let status = command
        .arg("--target-dir")
        .arg(&target)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .expect("running cargo");
    assert!(status.success(), "building {packages:?} failed");
    target.join("debug")
}

/// The host and the plugin, built together in one cargo invocation, as the
/// editor and its vendored plugins are.
fn built_together() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        build(
            "world-plugins",
            &["world_plugin_host", "world_static_plugin"],
            &[],
        )
    })
}

/// The plugin built on its own, with engine crate features the host does
/// not have: its engine `TypeId`s differ from the host's.
fn built_separately() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        build(
            "world-plugins-separate",
            &["world_static_plugin"],
            &["world_static_plugin/separate-build"],
        )
    })
}

/// Run the host on the plugin library in `plugin_dir` in `mode`; its
/// `key=value` report.
fn host_report(plugin_dir: &Path, mode: &str) -> HashMap<String, String> {
    let host = built_together().join(format!("world_plugin_host{}", std::env::consts::EXE_SUFFIX));
    let library = plugin_dir.join(libloading::library_filename("world_static_plugin"));
    let output = Command::new(host)
        .arg(&library)
        .arg(mode)
        .output()
        .expect("running the host");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "host failed ({mode}):\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The issue: a plugin with its own static copy of the world crates
/// registers its class into its own registry; the host never sees it, so
/// the class cannot be a component of the host's World.
#[test]
fn a_statically_linked_plugin_cannot_register_world_components() {
    let report = host_report(built_together(), "");
    assert_eq!(report["plugin_sees_class"], "true", "{report:?}");
    assert_eq!(report["host_sees_class"], "false", "{report:?}");
    assert_eq!(report["host_reflects_class"], "false", "{report:?}");
    assert_eq!(report["same_component_id"], "false", "{report:?}");
}

/// The fix: attached to the host's world runtime, the plugin's class is in
/// the host's registry with the host's component id, and a live component
/// of the host's World: created, written through reflection, read back and
/// journaled like a built-in class.
#[test]
fn a_plugin_attached_to_the_world_runtime_registers_live_world_components() {
    let report = host_report(built_together(), "attach");
    assert_eq!(report["attached"], "true", "{report:?}");
    assert_eq!(report["plugin_sees_class"], "true", "{report:?}");
    assert_eq!(report["host_sees_class"], "true", "{report:?}");
    assert_eq!(report["host_reflects_class"], "true", "{report:?}");
    assert_eq!(report["same_component_id"], "true", "{report:?}");
    assert_eq!(report["live_charge"], "Some(7.5)", "{report:?}");
    assert_eq!(report["journal_changes"], "1", "{report:?}");
    assert_eq!(report["reloaded_charge"], "Some(3.25)", "{report:?}");
}

/// Component identity does not rest on `TypeId`: a plugin built in another
/// cargo invocation, whose engine `TypeId`s differ from the host's, shares
/// the runtime the same way.
#[test]
fn a_plugin_built_separately_shares_the_world_runtime() {
    let report = host_report(built_separately(), "attach");
    assert_eq!(
        report["same_engine_type_ids"], "false",
        "the builds really differ: {report:?}"
    );
    assert_eq!(report["attached"], "true", "{report:?}");
    assert_eq!(report["host_sees_class"], "true", "{report:?}");
    assert_eq!(report["host_reflects_class"], "true", "{report:?}");
    assert_eq!(report["same_component_id"], "true", "{report:?}");
    assert_eq!(report["live_charge"], "Some(7.5)", "{report:?}");
    assert_eq!(report["journal_changes"], "1", "{report:?}");
    assert_eq!(report["reloaded_charge"], "Some(3.25)", "{report:?}");
}

/// A plugin whose world runtime has another ABI refuses to attach rather
/// than misread it, and registers nothing in the host.
#[test]
fn a_plugin_refuses_a_world_runtime_of_another_abi() {
    let report = host_report(built_together(), "attach-bad-abi");
    assert_eq!(report["attached"], "false", "{report:?}");
    assert_eq!(report["host_sees_class"], "false", "{report:?}");
    assert_eq!(report["host_reflects_class"], "false", "{report:?}");
}
