//! The host's plugin catalog: one snapshot of every installed plugin, joined
//! with whether the open project enables it, the crate its registrations
//! come from, what it contributes, and the contracts it provides and
//! requires, each linked to the plugin on the other side.
//!
//! - Identity is the descriptor's (`pulsar_plugins::PluginDescriptor`, from
//!   `pulsar-plugin.toml`): one stable plugin id. A registration's origin
//!   (the package that submitted it) is provenance that maps to that id
//!   through the plugin's root crate (`pulsar_plugins::plugin_of`).
//! - Contributions come from the real registrations: every registration
//!   type summarizes itself (`pulsar_plugins::Contribution`) and is listed
//!   as a source by `pulsar_plugins::registration!`, so the catalog reads
//!   every type linked into the binary without depending on any of their
//!   crates. A render feature registration's summary carries each feature,
//!   the nodes it registered (`helio_feature_api::NodeRegistry::
//!   feature_nodes`) and the scene buffers its passes read
//!   (`FeatureRegistrar::consumes`, category [`READS`]); the engine builds
//!   its graphs from the same registrations (`plugin_features()`), so the
//!   node ids agree.
//! - Contracts: each requirement is shown with the plugin that meets it,
//!   or the activation problem naming consumer, contract and provider
//!   (`pulsar_plugins::Problem`); each scene buffer a feature reads is linked
//!   to the plugin whose `buffer` contract covers the key
//!   ([`ContractLink`]). A read of another plugin's buffer the reader does
//!   not require, or of a buffer nobody provides, is a [`CatalogProblem`].
//! - A registration whose origin is `engine:<package>`
//!   (`pulsar_plugins::engine_origin!()`) is the engine's
//!   ([`CatalogSnapshot::engine`]). Any other origin that is no installed
//!   plugin's root is unknown: listed in [`CatalogSnapshot::unknown`] with
//!   its origin and reported as a [`CatalogProblem::UnknownOrigin`], never
//!   guessed from a name nor taken for the engine's.
//! - Plugin libraries loaded at runtime join the same snapshot
//!   ([`crate::PluginManager::catalog`]).
//!
//! The Extensions viewer (`ui_plugin_manager`) reads only this snapshot:
//! its counts, filters, dependencies and enabled state cannot disagree.
//!
//! How others do it: IntelliJ's extension area lists every extension point
//! and each extension's plugin descriptor; Unreal's plugin browser shows
//! descriptors only, and VS Code's extensions view reads a hand-written
//! `contributes` manifest. Ours is IntelliJ's: contributions are read from
//! the registrations, so the view cannot drift from what is registered.

use std::collections::{BTreeMap, BTreeSet};

use pulsar_plugins::{
    ContractKind, PluginDescriptor, Problem, RequiredContract, Resolution, Summary,
};

use crate::PluginDetails;

/// The category of a scene buffer a render feature reads (a child of the
/// feature's summary; its id is the buffer key).
pub const READS: &str = "render.reads";

/// The category of a trait a contribution uses (a child of a role or a
/// native implementer, `pulsar_traits::sources::TRAIT_USE`).
pub const TRAIT_USE: &str = "traits.uses";

/// The category of a trait definition a plugin's folder serves
/// (`pulsar_traits::catalog::DEFINITION`).
pub const TRAIT_DEFINITION: &str = "traits.definition";

/// The category whose children are the scene buffers the frame defines
/// (the engine lists `helio_frame_api::FRAME_BUFFERS` under it): a feature
/// reads these without a requirement.
pub const FRAME_BUFFERS: &str = "render.frame_buffers";

/// One snapshot of the installed plugins and what each contributes.
#[derive(Clone, Debug)]
pub struct CatalogSnapshot {
    /// Every installed plugin, by id.
    pub plugins: Vec<CatalogPlugin>,
    /// Plugin libraries loaded at runtime (`PluginManager`), with what each
    /// reports.
    pub libraries: Vec<PluginDetails>,
    /// The engine's own contributions (`engine:<package>` origins).
    pub engine: Vec<Summary>,
    /// Registrations whose origin is neither an installed plugin's root nor
    /// the engine's, each with its origin as provenance (each is also a
    /// problem).
    pub unknown: Vec<Summary>,
    /// What does not hold together: unknown origins, duplicate ids, reads
    /// of buffers nobody provides or the reader does not require.
    pub problems: Vec<CatalogProblem>,
    /// The project's activation: its problems (each naming consumer,
    /// contract and provider) and the optional contracts it runs without.
    /// `None` with nothing activated (every linked plugin enabled).
    pub activation: Option<Resolution>,
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
    /// Each contract it requires, with who meets it.
    pub requirements: Vec<Requirement>,
    /// The installed plugins that require a contract it provides.
    pub required_by: Vec<&'static str>,
    /// The scene buffers its features read, each with its provider.
    pub reads: Vec<ContractLink>,
}

/// A required contract and who meets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub required: RequiredContract,
    /// The installed plugins providing the contract.
    pub providers: Vec<&'static str>,
    pub state: RequirementState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequirementState {
    /// An enabled plugin provides it.
    Met { provider: &'static str },
    /// Optional, and nothing enabled provides it: the plugin runs without.
    AbsentOptional,
    /// Not met: the activation problem naming consumer, contract and
    /// provider.
    Unmet(Problem),
    /// The plugin itself is disabled (by the project or its default): its
    /// requirements are not resolved.
    Inactive,
}

/// A scene buffer a feature reads, linked to its provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractLink {
    pub feature: String,
    pub key: String,
    pub provider: Provider,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provider {
    /// A plugin's `buffer` contract covers the key.
    Plugin {
        plugin: &'static str,
        contract: &'static str,
    },
    /// The frame defines it ([`FRAME_BUFFERS`]).
    Engine,
    /// Nothing installed provides it.
    Nobody,
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
    /// A feature reads a scene buffer that no installed plugin's contract
    /// covers and the engine does not publish.
    UnprovidedBuffer {
        plugin: &'static str,
        feature: String,
        key: String,
    },
    /// A plugin uses (implements, makes a role of) another plugin's trait
    /// without requiring a `trait` contract of it that covers the trait.
    UndeclaredTraitUse {
        plugin: &'static str,
        trait_id: String,
        owner: &'static str,
    },
    /// A plugin's folder serves a trait no `trait` contract of its
    /// descriptor covers: no other plugin can require it.
    UndeclaredTraitDefinition {
        plugin: &'static str,
        trait_id: String,
    },
    /// A feature reads another plugin's buffer contract its plugin does not
    /// require: with the provider disabled the read finds nothing, and
    /// activation cannot say why.
    UndeclaredRead {
        plugin: &'static str,
        feature: String,
        key: String,
        provider: &'static str,
        contract: &'static str,
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
                "`{category}` `{id}` comes from `{origin}`, which is no installed plugin's \
                 root and not the engine's"
            ),
            Self::DuplicateId {
                origin,
                category,
                id,
            } => write!(
                f,
                "`{origin}` registers two `{category}` contributions with id `{id}`"
            ),
            Self::UnprovidedBuffer {
                plugin,
                feature,
                key,
            } => write!(
                f,
                "feature {feature} of plugin {plugin} reads buffer `{key}`, which no installed \
                 plugin provides"
            ),
            Self::UndeclaredTraitUse {
                plugin,
                trait_id,
                owner,
            } => write!(
                f,
                "plugin {plugin} uses trait {trait_id} of plugin {owner}, but requires no trait                  contract of {owner} covering it"
            ),
            Self::UndeclaredTraitDefinition { plugin, trait_id } => write!(
                f,
                "plugin {plugin} serves trait {trait_id}, which none of its trait contracts                  provides"
            ),
            Self::UndeclaredRead {
                plugin,
                feature,
                key,
                provider,
                contract,
            } => write!(
                f,
                "feature {feature} of plugin {plugin} reads buffer `{key}` of contract \
                 {contract} ({provider}), which {plugin} does not require"
            ),
        }
    }
}

impl CatalogSnapshot {
    /// The catalog now: the installed plugins, the activated set and every
    /// linked registration. Plugin libraries join through
    /// [`crate::PluginManager::catalog`].
    pub fn snapshot() -> Self {
        Self::from_parts(
            pulsar_plugins::installed(),
            pulsar_plugins::contributions(),
            pulsar_plugins::active_resolution(),
        )
    }

    /// The catalog of `installed` plugins given every linked registration's
    /// summary and the activation: what [`Self::snapshot`] joins, for a host
    /// or a test that assembles its parts.
    pub fn from_parts(
        installed: &'static [PluginDescriptor],
        contributions: Vec<Summary>,
        activation: Option<Resolution>,
    ) -> Self {
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
        let frame: BTreeSet<String> = engine
            .iter()
            .filter(|summary| summary.category == FRAME_BUFFERS)
            .flat_map(|summary| summary.children.iter().map(|key| key.id.clone()))
            .collect();
        let enabled = |id: &str| match &activation {
            Some(resolution) => resolution.is_enabled(id),
            None => true,
        };
        let plugins = installed
            .iter()
            .map(|descriptor| {
                let contributions = by_plugin.remove(descriptor.id.as_str()).unwrap_or_default();
                let reads = reads_of(installed, &frame, &contributions);
                for link in &reads {
                    match link.provider {
                        Provider::Nobody => problems.push(CatalogProblem::UnprovidedBuffer {
                            plugin: &descriptor.id,
                            feature: link.feature.clone(),
                            key: link.key.clone(),
                        }),
                        Provider::Plugin { plugin, contract }
                            if plugin != descriptor.id
                                && !descriptor.requires.iter().any(|r| r.id == contract) =>
                        {
                            problems.push(CatalogProblem::UndeclaredRead {
                                plugin: &descriptor.id,
                                feature: link.feature.clone(),
                                key: link.key.clone(),
                                provider: plugin,
                                contract,
                            })
                        }
                        _ => {}
                    }
                }
                problems.extend(trait_problems(installed, descriptor, &contributions));
                CatalogPlugin {
                    descriptor,
                    enabled: enabled(&descriptor.id),
                    modules: vec![descriptor.root.as_str()],
                    requirements: requirements_of(
                        installed,
                        descriptor,
                        enabled(&descriptor.id),
                        activation.as_ref(),
                    ),
                    required_by: installed
                        .iter()
                        .filter(|other| {
                            other
                                .requires
                                .iter()
                                .any(|r| descriptor.provided(&r.id).is_some())
                        })
                        .map(|other| other.id.as_str())
                        .collect(),
                    reads,
                    contributions,
                }
            })
            .collect();
        Self {
            plugins,
            libraries: Vec::new(),
            engine,
            unknown,
            problems,
            activation,
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

    /// The activation problems (empty with nothing activated).
    pub fn activation_problems(&self) -> &[Problem] {
        self.activation
            .as_ref()
            .map_or(&[], |resolution| resolution.problems.as_slice())
    }
}

/// The installed plugin whose namespace holds `trait_id` (`<plugin id>.<Trait>`).
fn trait_owner(
    installed: &'static [PluginDescriptor],
    trait_id: &str,
) -> Option<&'static PluginDescriptor> {
    installed.iter().find(|plugin| {
        trait_id
            .strip_prefix(plugin.id.as_str())
            .is_some_and(|rest| rest.starts_with('.') && !rest[1..].contains('.'))
    })
}

/// A plugin's trait uses and definitions that its contracts do not declare.
fn trait_problems(
    installed: &'static [PluginDescriptor],
    descriptor: &'static PluginDescriptor,
    contributions: &[Summary],
) -> Vec<CatalogProblem> {
    let mut problems = Vec::new();
    for summary in contributions.iter().flat_map(Summary::flatten) {
        match summary.category {
            TRAIT_USE => {
                let Some(owner) = trait_owner(installed, &summary.id) else {
                    continue;
                };
                if owner.id == descriptor.id {
                    continue;
                }
                let declared = owner
                    .provider_of(ContractKind::Trait, &summary.id)
                    .is_some_and(|contract| {
                        descriptor.requires.iter().any(|r| r.id == contract.id)
                    });
                if !declared {
                    problems.push(CatalogProblem::UndeclaredTraitUse {
                        plugin: &descriptor.id,
                        trait_id: summary.id.clone(),
                        owner: &owner.id,
                    });
                }
            }
            TRAIT_DEFINITION => {
                if descriptor
                    .provider_of(ContractKind::Trait, &summary.id)
                    .is_none()
                {
                    problems.push(CatalogProblem::UndeclaredTraitDefinition {
                        plugin: &descriptor.id,
                        trait_id: summary.id.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    problems
}

/// Each scene buffer `contributions`' features read, with its provider.
fn reads_of(
    installed: &'static [PluginDescriptor],
    frame: &BTreeSet<String>,
    contributions: &[Summary],
) -> Vec<ContractLink> {
    contributions
        .iter()
        .filter(|summary| summary.category == "render.features")
        .flat_map(|registration| &registration.children)
        .flat_map(|feature| {
            feature
                .children
                .iter()
                .filter(|child| child.category == READS)
                .map(move |read| ContractLink {
                    feature: feature.id.clone(),
                    key: read.id.clone(),
                    provider: provider_of_buffer(installed, frame, &read.id),
                })
        })
        .collect()
}

fn provider_of_buffer(
    installed: &'static [PluginDescriptor],
    frame: &BTreeSet<String>,
    key: &str,
) -> Provider {
    if let Some((plugin, contract)) = installed.iter().find_map(|plugin| {
        plugin
            .provider_of(ContractKind::Buffer, key)
            .map(|contract| (plugin, contract))
    }) {
        return Provider::Plugin {
            plugin: &plugin.id,
            contract: &contract.id,
        };
    }
    if frame.contains(key) {
        Provider::Engine
    } else {
        Provider::Nobody
    }
}

fn requirements_of(
    installed: &'static [PluginDescriptor],
    descriptor: &'static PluginDescriptor,
    enabled: bool,
    activation: Option<&Resolution>,
) -> Vec<Requirement> {
    descriptor
        .requires
        .iter()
        .map(|required| {
            let providers: Vec<&'static str> = installed
                .iter()
                .filter(|plugin| plugin.provided(&required.id).is_some())
                .map(|plugin| plugin.id.as_str())
                .collect();
            let unmet = activation.and_then(|resolution| {
                resolution
                    .problems
                    .iter()
                    .find(|problem| match problem {
                        Problem::Unprovided { consumer, contract }
                        | Problem::ProviderDisabled {
                            consumer, contract, ..
                        }
                        | Problem::Incompatible {
                            consumer, contract, ..
                        } => *consumer == descriptor.id && *contract == required.id,
                        _ => false,
                    })
                    .cloned()
            });
            let met_by = providers.iter().copied().find(|provider| {
                activation.is_none_or(|resolution| resolution.is_enabled(provider))
            });
            let state = match (unmet, met_by) {
                (Some(problem), _) => RequirementState::Unmet(problem),
                _ if !enabled => RequirementState::Inactive,
                (None, Some(provider)) => RequirementState::Met { provider },
                (None, None) if required.optional => RequirementState::AbsentOptional,
                (None, None) => RequirementState::Unmet(Problem::Unprovided {
                    consumer: descriptor.id.clone(),
                    contract: required.id.clone(),
                }),
            };
            Requirement {
                required: required.clone(),
                providers,
                state,
            }
        })
        .collect()
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
                        .filter(|child| child.category == "render.node")
                        .map(|node| node.id.as_str())
                        .collect(),
                )
            })
            .collect()
    }

    /// Whether any requirement is unmet.
    pub fn has_unmet_requirement(&self) -> bool {
        self.requirements
            .iter()
            .any(|requirement| matches!(requirement.state, RequirementState::Unmet(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_plugins::ProjectPlugins;

    fn descriptor(source: &str) -> PluginDescriptor {
        PluginDescriptor::parse(source).unwrap()
    }

    fn fog() -> PluginDescriptor {
        descriptor(
            r#"
            id = "p.fog"
            name = "Fog"
            root = "fog_plugin"
            requires = [{ id = "p.lights.rows" }, { id = "p.gi.probes", optional = true }]
            "#,
        )
    }

    fn lights() -> PluginDescriptor {
        descriptor(
            r#"
            id = "p.lights"
            name = "Lights"
            root = "lights_plugin"
            provides = [{ id = "p.lights.rows", kind = "buffer", members = ["lights"] }]
            "#,
        )
    }

    fn feature_with(reads: &[&str]) -> Summary {
        let mut registration =
            Summary::new("render.features", "p.fog", "Render features", "fog_plugin");
        registration.children = vec![Summary {
            children: [
                Summary::new("render.node", "fog.a", "A", "fog_plugin"),
                Summary::new("render.node", "fog.b", "B", "fog_plugin"),
            ]
            .into_iter()
            .chain(
                reads
                    .iter()
                    .map(|key| Summary::new(READS, *key, *key, "fog_plugin")),
            )
            .collect(),
            ..Summary::new("render.feature", "p.fog", "p.fog", "fog_plugin")
        }];
        registration
    }

    #[test]
    fn contributions_join_their_plugin_by_root_the_engine_or_unknown() {
        let installed: &'static [PluginDescriptor] = Box::leak(Box::new([fog(), lights()]));
        let catalog = CatalogSnapshot::from_parts(
            installed,
            vec![
                feature_with(&[]),
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
            None,
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
        assert_eq!(catalog.plugin("p.lights").unwrap().required_by, ["p.fog"]);
    }

    #[test]
    fn requirements_and_reads_link_to_their_provider() {
        let installed: &'static [PluginDescriptor] = Box::leak(Box::new([fog(), lights()]));
        // Everything enabled: the requirement is met by lights, the optional
        // one is absent, and each read names its provider.
        let resolution = pulsar_plugins::resolve(installed, &ProjectPlugins::default());
        let mut frame = Summary::new(
            FRAME_BUFFERS,
            "helio.frame",
            "Frame",
            "engine:engine_backend",
        );
        frame.children = vec![Summary::new(
            "render.frame_buffer",
            "scene_camera",
            "scene_camera",
            "engine:engine_backend",
        )];
        let catalog = CatalogSnapshot::from_parts(
            installed,
            vec![
                feature_with(&["lights", "scene_camera", "fog_media"]),
                frame,
            ],
            Some(resolution),
        );
        let fog = catalog.plugin("p.fog").unwrap();
        assert_eq!(
            fog.requirements
                .iter()
                .map(|r| &r.state)
                .collect::<Vec<_>>(),
            [
                &RequirementState::Met {
                    provider: "p.lights"
                },
                &RequirementState::AbsentOptional
            ]
        );
        assert_eq!(
            fog.reads
                .iter()
                .map(|link| (link.key.as_str(), &link.provider))
                .collect::<Vec<_>>(),
            [
                (
                    "lights",
                    &Provider::Plugin {
                        plugin: "p.lights",
                        contract: "p.lights.rows"
                    }
                ),
                ("scene_camera", &Provider::Engine),
                ("fog_media", &Provider::Nobody),
            ]
        );
        assert_eq!(
            catalog.problems,
            [CatalogProblem::UnprovidedBuffer {
                plugin: "p.fog",
                feature: "p.fog".into(),
                key: "fog_media".into()
            }]
        );

        // Lights disabled by the project: fog is disabled, and its
        // requirement carries the named problem.
        let resolution = pulsar_plugins::resolve(
            installed,
            &ProjectPlugins::default().with(&["p.lights"], false),
        );
        let catalog = CatalogSnapshot::from_parts(installed, Vec::new(), Some(resolution));
        let fog = catalog.plugin("p.fog").unwrap();
        assert!(!fog.enabled);
        assert!(fog.has_unmet_requirement());
        assert_eq!(
            catalog.activation_problems()[0].to_string(),
            "plugin p.fog needs contract p.lights.rows, provided by p.lights, which the \
             project disables; p.fog is disabled"
        );
    }

    #[test]
    fn reading_another_plugins_buffer_needs_a_requirement() {
        let mut undeclared = fog();
        undeclared.requires.clear();
        let installed: &'static [PluginDescriptor] = Box::leak(Box::new([undeclared, lights()]));
        let catalog = CatalogSnapshot::from_parts(installed, vec![feature_with(&["lights"])], None);
        assert_eq!(
            catalog.problems,
            [CatalogProblem::UndeclaredRead {
                plugin: "p.fog",
                feature: "p.fog".into(),
                key: "lights".into(),
                provider: "p.lights",
                contract: "p.lights.rows"
            }]
        );
    }
}
