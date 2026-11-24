//! A plugin library imported after the project was opened is gated by the
//! project's list like one installed before.

use pulsar_plugins::library::{import, FfiLibrary, FfiPlugin, FfiStr, LIBRARY_ABI};
use pulsar_plugins::ProjectPlugins;

const DESCRIPTOR: &str = "id = \"late.plugin\"\nname = \"Late\"\nroot = \"late_root\"\n";
const ROOT: &str = "late_root";

fn ffi(text: &'static str) -> FfiStr {
    FfiStr {
        ptr: text.as_ptr(),
        len: text.len(),
    }
}

#[test]
fn a_library_imported_after_the_project_opened_follows_the_projects_list() {
    // The project opens while only the editor's own plugins are installed; it
    // turns the library's plugin off.
    let project = ProjectPlugins::parse("[plugins]\n\"late.plugin\" = false\n");
    pulsar_plugins::activate_project(std::path::Path::new("project"), project);
    // Not installed yet: its registrations are not known to belong to a
    // plugin, so the gate lets them through.
    assert!(pulsar_plugins::origin_enabled(ROOT));

    let plugins: &'static [FfiPlugin] = Box::leak(Box::new([FfiPlugin {
        descriptor: ffi(DESCRIPTOR),
        root: ffi(ROOT),
        entries: std::ptr::NonNull::<pulsar_plugins::library::FfiEntry>::dangling().as_ptr(),
        len: 0,
    }]));
    let library = FfiLibrary {
        abi: LIBRARY_ABI,
        plugins: plugins.as_ptr(),
        len: plugins.len(),
    };
    // SAFETY: the arrays live for the process and the plugin has no entries.
    unsafe { import(&library) }.unwrap();

    // Resolved again over the larger set: the project's `false` applies.
    assert!(!pulsar_plugins::is_enabled("late.plugin"));
    assert!(!pulsar_plugins::origin_enabled(ROOT));
}
