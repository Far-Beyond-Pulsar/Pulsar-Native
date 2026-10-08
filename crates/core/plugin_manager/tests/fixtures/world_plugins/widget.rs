// The plugin's component class, shared by both fixture plugins.
use engine_class_derive::{engine_class, register_world_component};

/// A plugin-defined component: one reflected property.
#[engine_class(
    category = "Plugin",
    default,
    clone,
    debug,
    serialize,
    deserialize,
    no_register
)]
pub struct PluginWidget {
    #[property]
    pub charge: f32,
}

#[register_world_component]
impl PluginWidget {}
