//! A component class in a plugin library. Like every editor plugin, it links
//! its own static copy of the engine's world crates; its registration reaches
//! the host only once it attaches to the host's world runtime.

include!("../../widget.rs");

// The editor's plugin contract: the entry point the plugin manager calls to
// attach this library's world crates to the editor's runtimes.
pulsar_world_registry::export_world_runtime_attach!();

/// Whether the registry this library sees holds the class.
#[no_mangle]
pub extern "C" fn plugin_registry_has_widget() -> bool {
    pulsar_world_registry::registered_world_component_classes().any(|class| class == "PluginWidget")
}

/// The widget's component id as this library sees it.
#[no_mangle]
pub extern "C" fn plugin_widget_component_id() -> u64 {
    pulsar_scenedb::component_id::<PluginWidget>().0 as u64
}

/// A hash of this library's `TypeId` for an engine type, to tell whether
/// the host's `TypeId`s are the same as this library's.
#[no_mangle]
pub extern "C" fn plugin_engine_type_hash() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::any::TypeId::of::<pulsar_scenedb::World>().hash(&mut hasher);
    hasher.finish()
}
