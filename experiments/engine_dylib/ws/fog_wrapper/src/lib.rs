// Link the shared dylib so the crates it contains are NOT linked again.
use sdk_dylib as _;
use fog_plugin as _; // the real plugin root: its `installed!` registration + build
// Exactly what fog_plugin's `dll` feature does in its own lib.rs.
plugin_editor_api::export_library_plugin!();
#[path = "../../probe/probe.rs"]
mod probe;
