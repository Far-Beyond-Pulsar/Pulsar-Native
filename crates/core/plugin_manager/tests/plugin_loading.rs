//! Integration test: load a compiled plugin DLL through the real engine loader.
//!
//! Loads a purpose-built dynamic library fixture and verifies the real loader
//! contract without depending on an untracked, platform-specific plugin file.
//!
//! Run: cargo test -p plugin_manager --test plugin_loading -- --nocapture

use plugin_editor_api::VersionInfo;
use plugin_manager::{PermanentLibrary, PermanentLibraryLoadError};
use std::path::PathBuf;

fn plugin_path() -> PathBuf {
    let deps_dir = std::env::current_exe()
        .expect("current test executable path")
        .parent()
        .expect("test executable directory")
        .to_path_buf();
    let prefix = format!("{}plugin_loader_fixture", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let exact = deps_dir.join(format!("{prefix}{suffix}"));
    if exact.is_file() {
        return exact;
    }

    let candidates: Vec<PathBuf> = std::fs::read_dir(&deps_dir)
        .expect("read Cargo dependency artifact directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(suffix))
        })
        .collect();

    assert_eq!(
        candidates.len(),
        1,
        "expected one plugin loader fixture in '{}', found {candidates:?}",
        deps_dir.display(),
    );
    candidates.into_iter().next().unwrap()
}

#[test]
fn loads_version_matches_engine() {
    let lib = PermanentLibrary::new(plugin_path()).expect("failed to load plugin fixture");

    let version_fn: libloading::Symbol<unsafe extern "C" fn() -> VersionInfo> =
        unsafe { lib.get(b"_plugin_version") }.expect("_plugin_version symbol");

    let plugin_version = unsafe { version_fn() };
    let engine_version = VersionInfo::current();

    assert!(
        engine_version.is_compatible(&plugin_version),
        "Plugin version {:?} incompatible with engine {:?}",
        plugin_version,
        engine_version,
    );

    eprintln!(
        "✅ Plugin version {:?} compatible with engine {:?}",
        plugin_version, engine_version,
    );
}

#[test]
fn exports_required_symbols() {
    let lib = PermanentLibrary::new(plugin_path()).expect("failed to load plugin fixture");

    // _plugin_version
    let _: libloading::Symbol<unsafe extern "C" fn() -> VersionInfo> =
        unsafe { lib.get(b"_plugin_version") }.expect("_plugin_version symbol");

    // _plugin_create
    let _: libloading::Symbol<
        unsafe extern "C" fn(
            *const std::ffi::c_void,
        ) -> &'static mut dyn plugin_editor_api::EditorPluginFull,
    > = unsafe { lib.get(b"_plugin_create") }.expect("_plugin_create symbol");

    // _plugin_init_globals
    let _: libloading::Symbol<unsafe extern "C" fn(*const std::ffi::c_void)> =
        unsafe { lib.get(b"_plugin_init_globals") }.expect("_plugin_init_globals symbol");

    eprintln!("✅ All 3 required FFI symbols found");
}

#[test]
fn missing_plugin_reports_the_real_open_error() {
    let missing = std::env::temp_dir().join(format!(
        "missing_plugin_loader_fixture{}",
        std::env::consts::DLL_SUFFIX
    ));

    let error = PermanentLibrary::new(&missing).expect_err("missing library must fail");
    assert!(
        matches!(error, PermanentLibraryLoadError::Open { .. }),
        "expected an open error, got {error:?}"
    );
    assert!(
        error.to_string().contains(&missing.display().to_string()),
        "error must identify the missing library: {error}"
    );
}
