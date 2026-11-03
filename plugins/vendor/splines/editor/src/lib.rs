//! Splines (`pulsar.splines`): the editor plugin.
//!
//! The plugin's identity ([`SplinesPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`splines_component`), which this crate links: the editor links this
//! crate (`use splines_plugin as _`) and knows no splines type. The
//! inspector edits the components through their reflected properties.
//!
//! The spline tool mode, panel and assistant tools are still the level
//! editor's (`ui_level_editor`), acting through the component's methods.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use splines_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct SplinesPlugin;

impl SplinesPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.splines";
}

impl EditorPlugin for SplinesPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Splines".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description:
                "Authored curves: their component, its methods and the editor lines that show them"
                    .into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        Vec::new()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_the_plugin_id() {
        assert_eq!(SplinesPlugin.metadata().id.as_str(), "pulsar.splines");
    }
}
