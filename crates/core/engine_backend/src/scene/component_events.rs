//! Events a component raises that its assets declare: a skeletal mesh's
//! animation notifies, declared in the animation assets it plays. A
//! scripting editor (the Blueprint palette) asks for the events of a class
//! of component given the assets an instance names; the component's crate
//! answers through a [`ComponentEventsRegistration`], so neither the editor
//! nor the engine names the component's crate.
//!
//! How others do it: Unreal's AnimNotifies surface as events on the
//! AnimBlueprint (the animation module's own graph); Unity's Animation
//! Events call methods by name. Ours is the class's own catalogue, reached
//! by class name.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pulsar_events::gamma::EventDescriptor;

/// The events a class of component raises for some of its assets, with the
/// files that could not be read.
#[derive(Debug, Default)]
pub struct ComponentEventCatalog {
    pub events: Vec<(String, EventDescriptor)>,
    pub diagnostics: Vec<(PathBuf, String)>,
}

/// A component class's asset-declared events, submitted by its crate.
pub struct ComponentEventsRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// The component's class name.
    pub class_name: &'static str,
    /// The events of the assets with these ids (as an instance names them)
    /// in the project at `root`.
    pub events: fn(root: &Path, assets: &HashSet<String>) -> ComponentEventCatalog,
}

inventory::collect!(ComponentEventsRegistration);

impl pulsar_plugins::Registration for ComponentEventsRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

/// The events components of `class_name` raise for `assets` in the project
/// at `root`; empty when no enabled plugin registers the class's.
pub fn component_asset_events(
    class_name: &str,
    root: &Path,
    assets: &HashSet<String>,
) -> ComponentEventCatalog {
    pulsar_plugins::registered::<ComponentEventsRegistration>()
        .find(|registration| registration.class_name == class_name)
        .map(|registration| (registration.events)(root, assets))
        .unwrap_or_default()
}
