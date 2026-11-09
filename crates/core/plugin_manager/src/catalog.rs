//! The host's plugin catalog: one snapshot of every installed plugin, joined
//! with whether the open project enables it, the crate its registrations
//! come from and what it contributes.
//!
//! - Identity is the descriptor's (`pulsar_plugins::PluginDescriptor`, from
//!   `pulsar-plugin.toml`): one stable plugin id. A registration's origin
//!   (the package that submitted it) is provenance that maps to that id
//!   through the plugin's root crate (`pulsar_plugins::plugin_of`).
//! - Contributions come from the real registrations: every registration
//!   type summarizes itself (`pulsar_plugins::Contribution`) and is listed
//!   as a source by `pulsar_plugins::registration!`, so the catalog reads
//!   every type linked into the binary without depending on any of their
//!   crates. A render feature registration's summary carries each feature
//!   and the nodes it registered (`helio_feature_api::NodeRegistry::
//!   feature_nodes`); the engine builds its graphs from the same
//!   registrations (`plugin_features()`), so the node ids agree.
//! - A registration whose origin is `engine:<package>`
//!   (`pulsar_plugins::engine_origin!()`) is the engine's
//!   ([`PluginCatalog::engine`]). Any other origin that is no installed
//!   plugin's root (a crate registering outside its plugin's root, a typo)
//!   is unknown: listed in [`PluginCatalog::unknown`] with its origin and
//!   reported as a [`CatalogProblem::UnknownOrigin`], never guessed from a
//!   name nor taken for the engine's.
//!
//! This is the static (linked) half. Plugin libraries loaded at runtime
//! (`PluginManager`) join the same snapshot when dynamic plugins expose the
//! root `build` across the library boundary (Pulsar-Native#1608, phase 4).
//!
//! How others do it: IntelliJ's extension area lists every extension point
//! and each extension's plugin descriptor; Unreal's plugin browser shows
//! descriptors only, and VS Code's extensions view reads a hand-written
//! `contributes` manifest. Ours is IntelliJ's: contributions are read from
//! the registrations, so the view cannot drift from what is registered.

use std::collections::{BTreeMap, BTreeSet};

use pulsar_plugins::{PluginDescriptor, Summary};

/// One snapshot of the installed plugins and what each contributes.
#[derive(Clone, Debug)]
pub struct PluginCatalog {
    /// Every installed plugin, by id.
    pub plugins: Vec<CatalogPlugin>,
    /// The engine's own contributions (`engine:<package>` origins).
    pub engine: Vec<Summary>,
    /// Registrations whose origin is neither an installed plugin's root nor
    /// the engine's, each with its origin as provenance (each is also a
    /// problem).
    pub unknown: Vec<Summary>,
    /// What does not hold together: unknown origins, a plugin registering
    /// two things of a category under one id.
    pub problems: Vec<CatalogProblem>,
}

/// An installed plugin, as the catalog shows it.
#[derive(Clone, Debug)]
pub struct CatalogPlugin {
    pub descriptor: &'static PluginDescriptor,
    /// Whether the open project enables it (with nothing activated, every
    /// installed plugin is).
    pub enabled: bool,
    /// The crates its registrations come from: its root crate.
    pub modules: Vec<&'static str>,
    /// Its registrations, by category then id.
    pub contributions: Vec<Summary>,
}

/// Something in the catalog that does not hold together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogProblem {
    /// A registration from an origin that is no installed plugin's root and
    /// not the engine's: the gate cannot skip it with a plugin.
    UnknownOrigin {
        origin: &'static str,
        category: &'static str,
        id: String,
    },
    /// Two registrations of one category with one id from one origin.
    DuplicateId {
        origin: &'static str,
        category: &'static str,
        id: String,
    },
}

impl std::fmt::Display for CatalogProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownOrigin {
                origin,
                category,
                id,
            } => write!(
                f,
                "`{category}` `{id}` comes from `{origin}`, which is no installed plugin's                  root and not the engine's"
            ),
            Self::DuplicateId {
                origin,
                category,
                id,
            } => write!(
                f,
                "`{origin}` registers two `{category}` contributions with id `{id}`"
            ),
        }
    }
}

impl PluginCatalog {
    /// The catalog now: the installed plugins, the activated set and every
    /// linked registration.
    pub fn snapshot() -> Self {
        Self::from_parts(pulsar_plugins::installed(), pulsar_plugins::contributions())
    }

    fn from_parts(installed: &'static [PluginDescriptor], contributions: Vec<Summary>) -> Self {
        let mut by_plugin: BTreeMap<&str, Vec<Summary>> = BTreeMap::new();
        let mut engine = Vec::new();
        let mut unknown = Vec::new();
        let mut seen = BTreeSet::new();
        let mut problems = Vec::new();
        for summary in contributions {
            if !seen.insert((summary.origin, summary.category, summary.id.clone())) {
                problems.push(CatalogProblem::DuplicateId {
                    origin: summary.origin,
                    category: summary.category,
                    id: summary.id.clone(),
                });
            }
            if summary
                .origin
                .starts_with(pulsar_plugins::ENGINE_ORIGIN_PREFIX)
            {
                engine.push(summary);
            } else if let Some(plugin) = installed.iter().find(|plugin| plugin.owns(summary.origin))
            {
                by_plugin.entry(&plugin.id).or_default().push(summary);
            } else {
                problems.push(CatalogProblem::UnknownOrigin {
                    origin: summary.origin,
                    category: summary.category,
                    id: summary.id.clone(),
                });
                unknown.push(summary);
            }
        }
        let plugins = installed
            .iter()
            .map(|descriptor| CatalogPlugin {
                descriptor,
                enabled: pulsar_plugins::is_enabled(&descriptor.id),
                modules: vec![descriptor.root.as_str()],
                contributions: by_plugin.remove(descriptor.id.as_str()).unwrap_or_default(),
            })
            .collect();
        Self {
            plugins,
            engine,
            unknown,
            problems,
        }
    }

    /// The installed plugin `id`.
    pub fn plugin(&self, id: &str) -> Option<&CatalogPlugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.descriptor.id == id)
    }

    /// The plugin whose root is `origin` (the package a registration names).
    pub fn owner_of(&self, origin: &str) -> Option<&CatalogPlugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.descriptor.owns(origin))
    }
}

impl CatalogPlugin {
    /// Its registrations of `category`.
    pub fn of(&self, category: &str) -> impl Iterator<Item = &Summary> + '_ {
        let category = category.to_string();
        self.contributions
            .iter()
            .filter(move |summary| summary.category == category)
    }

    /// Its render features, each with the node kinds it registered, by
    /// feature id.
    pub fn feature_nodes(&self) -> BTreeMap<&str, Vec<&str>> {
        self.of("render.features")
            .flat_map(|registration| &registration.children)
            .map(|feature| {
                (
                    feature.id.as_str(),
                    feature
                        .children
                        .iter()
                        .map(|node| node.id.as_str())
                        .collect(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(id: &str, root: &str) -> PluginDescriptor {
        PluginDescriptor::parse(&format!(
            "id = \"{id}\"\nname = \"{id}\"\nroot = \"{root}\""
        ))
        .unwrap()
    }

    #[test]
    fn contributions_join_their_plugin_by_root_the_engine_or_unknown() {
        let installed: &'static [PluginDescriptor] =
            Box::leak(Box::new([descriptor("p.fog", "fog_plugin")]));
        let mut feature = Summary::new("render.features", "p.fog", "Render features", "fog_plugin");
        feature.children = vec![Summary {
            children: vec![
                Summary::new("render.node", "fog.a", "A", "fog_plugin"),
                Summary::new("render.node", "fog.b", "B", "fog_plugin"),
            ],
            ..Summary::new("render.feature", "p.fog", "p.fog", "fog_plugin")
        }];
        let catalog = PluginCatalog::from_parts(
            installed,
            vec![
                feature,
                Summary::new("settings.group", "fog", "Fog", "fog_plugin"),
                Summary::new("settings.group", "fog", "Fog", "fog_plugin"),
                // A registration from a plugin's other crate is not the
                // plugin's: it keeps its origin, unknown.
                Summary::new("component.class", "FogVolume", "FogVolume", "fog_component"),
                Summary::new(
                    "render.host_part",
                    "lines",
                    "lines",
                    "engine:engine_backend",
                ),
            ],
        );
        let fog = catalog.plugin("p.fog").unwrap();
        assert_eq!(fog.modules, ["fog_plugin"]);
        assert_eq!(fog.contributions.len(), 3);
        assert_eq!(
            fog.feature_nodes(),
            BTreeMap::from([("p.fog", vec!["fog.a", "fog.b"])])
        );
        assert_eq!(catalog.engine.len(), 1);
        assert_eq!(catalog.unknown.len(), 1);
        assert_eq!(
            catalog.problems,
            [
                CatalogProblem::DuplicateId {
                    origin: "fog_plugin",
                    category: "settings.group",
                    id: "fog".into()
                },
                CatalogProblem::UnknownOrigin {
                    origin: "fog_component",
                    category: "component.class",
                    id: "FogVolume".into()
                },
            ]
        );
        assert!(catalog.owner_of("fog_plugin").is_some());
        assert!(catalog.owner_of("fog_component").is_none());
    }
}
