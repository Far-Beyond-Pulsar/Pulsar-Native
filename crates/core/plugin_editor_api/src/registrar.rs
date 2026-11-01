//! The registrar authoring API: one `build` call instead of a stack of
//! extension traits.
//!
//! ```rust,ignore
//! #[derive(Default)]
//! struct MyPlugin;
//!
//! impl Plugin for MyPlugin {
//!     fn manifest(&self) -> PluginManifest {
//!         PluginManifest::new("com.example.my", "My Plugin", "0.1.0", "me")
//!             .requires(PluginDependency::new("com.pulsar.physics", "^0.1"))
//!     }
//!
//!     fn build(&self, p: &mut PluginBuilder) {
//!         p.files().file_type(my_file_type());
//!         p.editors().editor(my_editor_metadata(), |registry| {
//!             registry.register::<MyEditorHandle>("my-editor", "myext");
//!         });
//!         p.components().class(definition, || Box::new(MyComponent::default()));
//!     }
//! }
//!
//! export_plugin_v2!(MyPlugin);
//! ```
//!
//! A new capability adds a section to [`PluginBuilder`]; it does not add a
//! trait to [`EditorPluginFull`], so the plugin ABI does not change with it.
//! [`PluginAdapter`] presents a registrar plugin to the host through the
//! existing traits, so the host loads it like any other plugin.

use std::sync::{Arc, Mutex, OnceLock};

use crate::components::{ComponentDefinition, ComponentFactory, EditorPluginComponents};
use crate::editor_element::{EditorFactoryRegistry, EditorPluginEditor};
use crate::file_types::FileTypeDefinition;
use crate::manifest::PluginManifest;
use crate::metadata::{EditorMetadata, PluginMetadata};
use crate::plugin::EditorPlugin;
use crate::statusbar::{EditorPluginStatusbar, StatusbarButtonDefinition};
use crate::subsystems::{EditorPluginSubsystems, Subsystem};

/// A plugin written against the registrar API.
///
/// Must be `Default`: the plugin library constructs it.
pub trait Plugin: Send + Sync + 'static {
    /// Identity and dependencies. Called before any other method, and before
    /// the host has loaded the plugin's dependencies, so it must not touch
    /// another plugin.
    fn manifest(&self) -> PluginManifest;

    /// Register everything this plugin provides. Called once, after the
    /// plugin's dependencies have loaded.
    fn build(&self, p: &mut PluginBuilder);

    /// One-time initialisation after `build`.
    fn on_load(&mut self) {}
}

type EditorFactoryFn = Arc<dyn Fn(&mut EditorFactoryRegistry) + Send + Sync>;
type ComponentMaker = Arc<dyn Fn() -> Box<dyn pulsar_reflection::EngineClass> + Send + Sync>;

/// Everything a plugin registered in `build`.
#[derive(Default)]
pub struct Registration {
    file_types: Vec<FileTypeDefinition>,
    editors: Vec<EditorMetadata>,
    editor_factories: Vec<EditorFactoryFn>,
    component_definitions: Vec<ComponentDefinition>,
    component_makers: Vec<(String, ComponentMaker)>,
    statusbar_buttons: Vec<StatusbarButtonDefinition>,
    subsystems: Mutex<Vec<Box<dyn Subsystem>>>,
}

/// Handed to [`Plugin::build`]. One accessor per kind of thing a plugin
/// provides.
#[derive(Default)]
pub struct PluginBuilder {
    registration: Registration,
}

impl PluginBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn files(&mut self) -> FilesSection<'_> {
        FilesSection(&mut self.registration)
    }

    pub fn editors(&mut self) -> EditorsSection<'_> {
        EditorsSection(&mut self.registration)
    }

    pub fn components(&mut self) -> ComponentsSection<'_> {
        ComponentsSection(&mut self.registration)
    }

    pub fn statusbar(&mut self) -> StatusbarSection<'_> {
        StatusbarSection(&mut self.registration)
    }

    pub fn subsystems(&mut self) -> SubsystemsSection<'_> {
        SubsystemsSection(&mut self.registration)
    }

    pub fn finish(self) -> Registration {
        self.registration
    }
}

pub struct FilesSection<'a>(&'a mut Registration);

impl FilesSection<'_> {
    pub fn file_type(&mut self, file_type: FileTypeDefinition) -> &mut Self {
        self.0.file_types.push(file_type);
        self
    }
}

pub struct EditorsSection<'a>(&'a mut Registration);

impl EditorsSection<'_> {
    /// Declare an editor, and register its factories (and panel restorers)
    /// when the host asks for them.
    pub fn editor(
        &mut self,
        metadata: EditorMetadata,
        register: impl Fn(&mut EditorFactoryRegistry) + Send + Sync + 'static,
    ) -> &mut Self {
        self.0.editors.push(metadata);
        self.0.editor_factories.push(Arc::new(register));
        self
    }
}

pub struct ComponentsSection<'a>(&'a mut Registration);

impl ComponentsSection<'_> {
    /// A component class. `definition.id` is the class name.
    pub fn class(
        &mut self,
        definition: ComponentDefinition,
        make: impl Fn() -> Box<dyn pulsar_reflection::EngineClass> + Send + Sync + 'static,
    ) -> &mut Self {
        self.0
            .component_makers
            .push((definition.id.clone(), Arc::new(make)));
        self.0.component_definitions.push(definition);
        self
    }
}

pub struct StatusbarSection<'a>(&'a mut Registration);

impl StatusbarSection<'_> {
    pub fn button(&mut self, button: StatusbarButtonDefinition) -> &mut Self {
        self.0.statusbar_buttons.push(button);
        self
    }
}

pub struct SubsystemsSection<'a>(&'a mut Registration);

impl SubsystemsSection<'_> {
    pub fn subsystem(&mut self, subsystem: Box<dyn Subsystem>) -> &mut Self {
        self.0
            .subsystems
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .push(subsystem);
        self
    }
}

/// Presents a [`Plugin`] to the host as an [`EditorPluginFull`](crate::plugin::EditorPluginFull).
///
/// `build` runs on first use and its result is kept.
pub struct PluginAdapter<P: Plugin> {
    plugin: P,
    manifest: OnceLock<PluginManifest>,
    registration: OnceLock<Registration>,
}

impl<P: Plugin + Default> Default for PluginAdapter<P> {
    fn default() -> Self {
        Self {
            plugin: P::default(),
            manifest: OnceLock::new(),
            registration: OnceLock::new(),
        }
    }
}

impl<P: Plugin> PluginAdapter<P> {
    fn registration(&self) -> &Registration {
        self.registration.get_or_init(|| {
            let mut builder = PluginBuilder::new();
            self.plugin.build(&mut builder);
            builder.finish()
        })
    }

    fn cached_manifest(&self) -> &PluginManifest {
        self.manifest.get_or_init(|| self.plugin.manifest())
    }
}

impl<P: Plugin> EditorPlugin for PluginAdapter<P> {
    fn manifest(&self) -> PluginManifest {
        self.cached_manifest().clone()
    }

    fn metadata(&self) -> PluginMetadata {
        self.cached_manifest().metadata()
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        self.registration().file_types.clone()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        self.registration().editors.clone()
    }

    fn on_load(&mut self) {
        self.plugin.on_load();
    }
}

impl<P: Plugin> EditorPluginEditor for PluginAdapter<P> {
    fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
        for register in &self.registration().editor_factories {
            register(registry);
        }
    }
}

impl<P: Plugin> EditorPluginStatusbar for PluginAdapter<P> {
    fn statusbar_buttons(&self) -> Vec<StatusbarButtonDefinition> {
        self.registration().statusbar_buttons.clone()
    }
}

impl<P: Plugin> crate::ai::EditorPluginAi for PluginAdapter<P> {}

impl<P: Plugin> EditorPluginComponents for PluginAdapter<P> {
    fn component_definitions(&self) -> Vec<ComponentDefinition> {
        self.registration().component_definitions.clone()
    }

    fn component_factories(&self) -> Vec<(String, ComponentFactory)> {
        self.registration()
            .component_makers
            .iter()
            .map(|(name, make)| {
                let make = Arc::clone(make);
                (name.clone(), Box::new(move || make()) as ComponentFactory)
            })
            .collect()
    }
}

impl<P: Plugin> EditorPluginSubsystems for PluginAdapter<P> {
    fn subsystems(&self) -> Vec<Box<dyn Subsystem>> {
        std::mem::take(
            &mut *self
                .registration()
                .subsystems
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        )
    }
}

impl<P: Plugin> crate::scripting::EditorPluginScripting for PluginAdapter<P> {}

impl<P: Plugin> crate::plugin::EditorPluginFull for PluginAdapter<P> {}

/// Export a registrar [`Plugin`] from a plugin library.
#[macro_export]
macro_rules! export_plugin_v2 {
    ($plugin_type:ty) => {
        $crate::export_plugin!($crate::registrar::PluginAdapter<$plugin_type>);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::PluginDependency;
    use crate::plugin::EditorPlugin as _;

    #[derive(Default)]
    struct Sample;

    impl Plugin for Sample {
        fn manifest(&self) -> PluginManifest {
            PluginManifest::new("test.sample", "Sample", "0.2.0", "tests")
                .requires(PluginDependency::new("test.base", "^0.1"))
        }

        fn build(&self, p: &mut PluginBuilder) {
            p.editors().editor(
                EditorMetadata {
                    id: crate::identifiers::EditorId::new("sample-editor"),
                    display_name: "Sample".into(),
                    supported_file_types: Vec::new(),
                },
                |_registry| {},
            );
        }
    }

    #[test]
    fn adapter_presents_manifest_metadata_and_registrations() {
        let adapter = PluginAdapter::<Sample>::default();
        let manifest = adapter.manifest();
        assert_eq!(manifest.id.as_str(), "test.sample");
        assert_eq!(manifest.requires[0].id.as_str(), "test.base");
        assert_eq!(adapter.metadata().name, "Sample");
        assert_eq!(adapter.editors().len(), 1);
        assert!(adapter.file_types().is_empty());
    }

    #[test]
    fn build_runs_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static BUILDS: AtomicUsize = AtomicUsize::new(0);

        #[derive(Default)]
        struct Counting;
        impl Plugin for Counting {
            fn manifest(&self) -> PluginManifest {
                PluginManifest::new("test.counting", "Counting", "0.1.0", "tests")
            }
            fn build(&self, _p: &mut PluginBuilder) {
                BUILDS.fetch_add(1, Ordering::SeqCst);
            }
        }

        let adapter = PluginAdapter::<Counting>::default();
        adapter.file_types();
        adapter.editors();
        adapter.file_types();
        assert_eq!(BUILDS.load(Ordering::SeqCst), 1);
    }
}
