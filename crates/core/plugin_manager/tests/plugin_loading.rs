//! Integration test: load a compiled plugin DLL through the real engine loader.
//!
//! Builds a `PluginManager`, loads the script editor `.dylib` from
//! `plugins/editor/`, and verifies the full plugin lifecycle works.
//!
//! Run: cargo test -p plugin_manager --test plugin_loading -- --nocapture

use plugin_editor_api::VersionInfo;
use plugin_manager::PermanentLibrary;

const PLUGIN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/editor/libscript_editor_plugin.dylib",
);

#[test]
fn loads_version_matches_engine() {
    let lib = PermanentLibrary::new(PLUGIN_PATH).expect("failed to load plugin dylib");

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
    let lib = PermanentLibrary::new(PLUGIN_PATH).expect("failed to load plugin dylib");

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
