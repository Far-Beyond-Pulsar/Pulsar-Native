//! The trait proof's fixture plugin (`pulsar.test_traits`,
//! `plugins/installed/tests/traits.rs`): one root crate that defines a trait
//! and a role and implements the trait with a component.
//!
//! - The trait `pulsar.test_traits.Greeter` is a file of the plugin's folder
//!   (`plugins/pulsar.test_traits/traits/greeter.trait.json`), served by its
//!   `engine_fs::FileProvider`; the descriptor provides it as a `trait`
//!   contract.
//! - The role `pulsar.test_traits.greeter` hands a level's implementer of the
//!   trait a job; its setting key in the level's `world_settings` is
//!   `greeter`.
//! - `GreeterComponent` implements the trait natively: the trait's `greet`
//!   is the component's by-name `greet` method (the one a script or the
//!   editor calls).
//!
//! Everything is registered here, in `build`, so a project that disables the
//! plugin has none of it.

pulsar_plugins::installed!("pulsar-plugin.toml", build);

use std::sync::Arc;

use engine_backend::scene::{ClassSpec, ComponentsBuilder};
use engine_class_derive::{engine_class, world_component};
use pulsar_traits::{RoleRegistration, RoleStart};

/// The id of the trait the plugin defines.
pub const GREETER_TRAIT: &str = "pulsar.test_traits.Greeter";
/// The id of the role the plugin registers.
pub const GREETER_ROLE: &str = "pulsar.test_traits.greeter";
/// The role's key in a level's `world_settings`.
pub const GREETER_SETTING: &str = "greeter";

/// A component that greets: it counts its greetings.
#[engine_class(category = "Test", default, clone, debug, serialize, deserialize)]
pub struct GreeterComponent {
    #[property]
    pub greetings: i64,
}

// The plugin's `build` registers the class.
#[world_component]
impl GreeterComponent {}

impl GreeterComponent {
    /// Greet once: the component method the trait's `greet` is bound to.
    pub fn greet(&mut self) {
        self.greetings += 1;
    }
}

/// The greeter's by-name methods (what a script, the editor and the trait
/// dispatcher call): `greet`.
fn greeter_methods() -> Vec<pulsar_reflection::MethodMetadata> {
    use pulsar_reflection::{EngineClass, MethodFlags, MethodMetadata};
    vec![MethodMetadata {
        name: "greet",
        display_name: "Greet".into(),
        category: Some("Greeter"),
        params: Vec::new(),
        return_type: None,
        flags: MethodFlags::NONE,
        caller: Box::new(
            |instance: &mut dyn EngineClass, _args: Vec<Box<dyn std::any::Any>>| {
                instance
                    .as_any_mut()
                    .downcast_mut::<GreeterComponent>()?
                    .greet();
                Some(Box::new(()))
            },
        ),
    }]
}

// A type attachment of the class (reflection's method table), as every
// component's methods are.
pulsar_reflection::inventory::submit! {
    pulsar_reflection::ComponentMethodRegistration {
        class_name: "GreeterComponent",
        methods: greeter_methods,
    }
}

/// The plugin's trait files, embedded: its folder's content.
const TRAIT_FILES: &[engine_fs::EmbeddedFile] = engine_fs::embedded_files!("content":
    "traits/greeter.trait.json",
);

fn open(folder: engine_fs::PluginFolder) -> Arc<dyn engine_fs::FolderSource> {
    Arc::new(engine_fs::MemoryFolder::with_files(folder, TRAIT_FILES))
}

/// Everything the plugin registers.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.components()
        .class::<GreeterComponent>(ClassSpec::default());

    // The folder serving the trait file.
    p.add(engine_fs::FileProvider {
        origin: p.origin(),
        open,
        in_game: engine_fs::InGame::Live,
    });

    // The role, spawned like the engine's character: queued at the end of
    // the frame the session starts.
    p.add(RoleRegistration {
        origin: p.origin(),
        id: GREETER_ROLE,
        label: "Greeter",
        trait_id: GREETER_TRAIT,
        setting_key: GREETER_SETTING,
        order: 100,
        start: RoleStart::EndOfFrame,
    });

    // The component implements the trait: `greet` binds to its method of the
    // same name.
    pulsar_traits::implements!(p, GreeterComponent: "pulsar.test_traits.Greeter");
}
