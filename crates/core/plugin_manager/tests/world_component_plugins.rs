//! Plugin component classes as World components (Pulsar-Native#1081).
//!
//! A plugin library linked statically carries its own copy of the engine's
//! world crates, so its classes register into registries the editor never
//! reads. Linked through `pulsar_world_dylib`, as the editor is, its classes
//! register into the editor's registries when it is loaded and are live
//! components of the editor's `World`.
//!
//! The host and both plugins are fixtures (`tests/fixtures/world_plugins`)
//! built in one cargo invocation, so the host and the shared plugin name the
//! same `pulsar_world_dylib`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// Build the host and both plugins once; returns `target/debug`.
fn built() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("world-plugins");
        let status = Command::new(cargo())
            .args([
                "build",
                "--quiet",
                "-p",
                "world_plugin_host",
                "-p",
                "world_static_plugin",
                "-p",
                "world_shared_plugin",
                "--target-dir",
            ])
            .arg(&target)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .status()
            .expect("running cargo");
        assert!(
            status.success(),
            "building the world plugin fixtures failed"
        );
        target.join("debug")
    })
}

/// Run the host on `plugin`; its `key=value` report.
fn host_report(plugin: &str) -> HashMap<String, String> {
    let dir = built();
    let libdir = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .args(["--print", "target-libdir"])
        .output()
        .expect("rustc --print target-libdir");
    let std_dir = PathBuf::from(String::from_utf8(libdir.stdout).unwrap().trim());
    // The host and the shared plugin load the world dylib and the dynamic
    // standard library from these directories.
    let search = std::env::join_paths([dir.join("deps"), dir.to_path_buf(), std_dir]).unwrap();
    let path_var = if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let library = dir.join(libloading::library_filename(plugin));
    let output =
        Command::new(dir.join(format!("world_plugin_host{}", std::env::consts::EXE_SUFFIX)))
            .arg(&library)
            .env(path_var, search)
            .output()
            .expect("running the host");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "host failed on {plugin}:\n{stdout}\n{}",
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
    let report = host_report("world_static_plugin");
    assert_eq!(report["plugin_sees_class"], "true", "{report:?}");
    assert_eq!(report["host_sees_class"], "false", "{report:?}");
    assert_eq!(report["same_component_id"], "false", "{report:?}");
}

/// The fix: linked through `pulsar_world_dylib`, the plugin's class is in
/// the host's registry with the host's component id, and a live component
/// of the host's World: created, written through reflection, read back and
/// journaled like a built-in class.
#[test]
fn a_plugin_linked_through_the_world_dylib_registers_live_world_components() {
    let report = host_report("world_shared_plugin");
    assert_eq!(report["plugin_sees_class"], "true", "{report:?}");
    assert_eq!(report["host_sees_class"], "true", "{report:?}");
    assert_eq!(report["same_component_id"], "true", "{report:?}");
    assert_eq!(report["live_charge"], "Some(7.5)", "{report:?}");
    assert_eq!(report["journal_changes"], "1", "{report:?}");
}

/// The cost of that linkage: the standard library is linked dynamically,
/// and a binary's `#[global_allocator]` then serves only the generic code
/// instantiated in that binary. Code compiled into `libstd` and the world
/// dylib allocates through `libstd`'s default allocator. An editor linked
/// this way would split its allocations between `TrackingAllocator` (the
/// memory panel) or the `dhat-heap` profiler and the system allocator.
#[test]
fn a_binary_linked_to_the_world_dylib_keeps_only_part_of_its_allocations() {
    let report = host_report("world_static_plugin");
    assert_eq!(
        report["library_allocation_reaches_host_allocator"], "false",
        "{report:?}"
    );
    assert_eq!(
        report["host_generic_allocation_reaches_host_allocator"], "true",
        "{report:?}"
    );
}
