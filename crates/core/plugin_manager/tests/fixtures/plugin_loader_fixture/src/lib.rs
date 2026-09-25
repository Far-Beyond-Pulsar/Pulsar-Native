use plugin_editor_api::VersionInfo;
use std::ffi::c_void;

#[no_mangle]
pub extern "C" fn _plugin_version() -> VersionInfo {
    VersionInfo::current()
}

#[no_mangle]
pub extern "C" fn _plugin_create(_app: *const c_void) -> *mut c_void {
    std::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn _plugin_init_globals(_theme: *const c_void) {}

// The host event bus entry point every `export_plugin!` plugin exports (#930).
plugin_editor_api::pulsar_events::export_host_bus_attach!();
