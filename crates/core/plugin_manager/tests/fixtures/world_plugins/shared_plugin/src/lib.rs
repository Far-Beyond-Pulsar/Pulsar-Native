//! A component class in a plugin linked through `pulsar_world_dylib`: its
//! registration goes into the host's registry when the host loads it.

use pulsar_world_dylib as _;

include!("../../widget.rs");

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
