//! Plugin dependencies across the real DLL boundary: manifests read from
//! libraries built with `export_plugin_v2!`, load order, rejection, and a
//! constructor that fails without unwinding into the host.
//!
//! The libraries are `tests/fixtures/dependency_plugins/{base,top,orphan}`;
//! `top` requires `base ^0.1` and `orphan` requires a plugin that is not
//! there.

use std::path::{Path, PathBuf};

use plugin_manager::{
    read_manifest, DependencyError, PermanentLibrary, PluginManager, PluginManagerError,
};

fn deps_dir() -> PathBuf {
    std::env::current_exe()
        .expect("current test executable path")
        .parent()
        .expect("test executable directory")
        .to_path_buf()
}

/// The built fixture library `name` (a crate name), which cargo writes into
/// the test's `deps` directory.
fn fixture(name: &str) -> PathBuf {
    let prefix = format!("{}{name}", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let dir = deps_dir();
    let exact = dir.join(format!("{prefix}{suffix}"));
    if exact.is_file() {
        return exact;
    }
    let found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read Cargo dependency artifact directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(suffix))
        })
        .collect();
    assert_eq!(found.len(), 1, "expected one {name} in {dir:?}: {found:?}");
    found.into_iter().next().unwrap()
}

/// Copies the three fixtures into a fresh directory with an integrity
/// manifest that allows them, and returns it with their paths in
/// `[base, top, orphan]` order.
fn install_dir() -> (tempfile::TempDir, [PathBuf; 3]) {
    let dir = tempfile::tempdir().expect("temp dir");
    let copy = |name: &str| -> PathBuf {
        let source = fixture(name);
        let target = dir.path().join(source.file_name().unwrap());
        std::fs::copy(&source, &target).expect("copy fixture");
        target
    };
    let paths = [
        copy("fixture_dep_base"),
        copy("fixture_dep_top"),
        copy("fixture_dep_orphan"),
    ];
    std::fs::write(
        dir.path().join("plugin_integrity.json"),
        r#"{"__allow_unlisted__": "true"}"#,
    )
    .expect("write integrity manifest");
    (dir, paths)
}

fn prepare(path: &Path) -> (PathBuf, Result<plugin_manager::PreparedPlugin, PluginManagerError>) {
    (path.to_path_buf(), PluginManager::prepare_plugin(path))
}

#[test]
fn a_library_exports_its_manifest_with_dependencies() {
    let top = PermanentLibrary::new(fixture("fixture_dep_top")).expect("load top");
    let manifest = read_manifest(&top).expect("top exports a manifest");
    assert_eq!(manifest.id.as_str(), "fixture.top");
    assert_eq!(manifest.requires.len(), 1);
    assert_eq!(manifest.requires[0].id.as_str(), "fixture.base");
    assert_eq!(manifest.requires[0].requirement.to_string(), "^0.1");
}

#[test]
fn prepared_plugins_come_back_dependencies_first() {
    let (_dir, [base, top, orphan]) = install_dir();
    // Dependent first on purpose: the order must come from the manifests.
    let ordered = PluginManager::order_prepared_plugins(vec![
        prepare(&top),
        prepare(&orphan),
        prepare(&base),
    ]);

    let names: Vec<(String, bool)> = ordered
        .iter()
        .map(|(path, result)| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (name, result.is_ok())
        })
        .collect();
    let position = |needle: &str| {
        names
            .iter()
            .position(|(name, _)| name.contains(needle))
            .unwrap_or_else(|| panic!("{needle} missing from {names:?}"))
    };

    assert!(names[position("base")].1 && names[position("top")].1, "{names:?}");
    assert!(position("base") < position("top"), "{names:?}");

    let (_, orphan_result) = &ordered[position("orphan")];
    match orphan_result {
        Err(PluginManagerError::DependencyUnmet { plugin, reason }) => {
            assert_eq!(plugin.as_str(), "fixture.orphan");
            assert!(reason.contains("fixture.absent"), "{reason}");
        }
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("orphan must not load"),
    }
    assert!(position("orphan") > position("top"), "rejected plugins come last");
}

#[test]
fn a_plugin_whose_constructor_fails_returns_none_instead_of_unwinding() {
    let library = PermanentLibrary::new(fixture("fixture_dep_base")).expect("load base");
    let create: libloading::Symbol<plugin_editor_api::PluginCreate> =
        unsafe { library.get(b"_plugin_create") }.expect("_plugin_create symbol");
    // A null theme pointer used to panic across the FFI boundary.
    let created = unsafe { create(std::ptr::null()) };
    assert!(created.is_none());
}

#[test]
fn dependency_errors_name_what_is_wrong() {
    let missing = DependencyError::Missing {
        dependency: plugin_editor_api::PluginDependency::new("a", "^1"),
    };
    assert!(missing.to_string().contains("not installed"));
}
