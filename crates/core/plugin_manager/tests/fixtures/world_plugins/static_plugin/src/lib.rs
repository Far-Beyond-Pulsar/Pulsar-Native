//! A component class in a plugin linked statically (the editor's plugins
//! before #1081). Its registration goes into this library's own copy of
//! `pulsar_world_registry`.

include!("../../widget.rs");

/// Whether this library's own registry holds the class.
#[no_mangle]
pub extern "C" fn plugin_registry_has_widget() -> bool {
    pulsar_world_registry::registered_world_component_classes().any(|class| class == "PluginWidget")
}

/// The widget's component id in this library's own id space.
#[no_mangle]
pub extern "C" fn plugin_widget_component_id() -> u64 {
    pulsar_scenedb::component_id::<PluginWidget>().0 as u64
}
