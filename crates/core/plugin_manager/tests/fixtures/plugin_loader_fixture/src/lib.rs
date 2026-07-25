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
