//! What a plugin provides to others and what it needs from them: its
//! **contracts** (`provides` and `requires` in `pulsar-plugin.toml`).
//!
//! A plugin depends on contracts, never on another plugin by name: a
//! requirement names a contract id (`pulsar.lights.rows`), and whichever
//! installed plugin provides that id satisfies it. Resolution
//! ([`crate::resolve`]) turns requirements into the enabled set and names
//! what fails: the consumer, the contract and its provider
//! ([`Problem`]). An optional requirement nobody provides is recorded
//! ([`crate::Resolution::absent_optional`]), not an error.
//!
//! Contract kinds ([`ContractKind`]) say what crosses between the plugins:
//!
//! | Kind | Members | Linked |
//! |---|---|---|
//! | `buffer` | Helio scene buffer keys (`BufferContract` names) the provider's components publish | yes: the consumer reads the key through the provider's api crate |
//! | `api` | crates other plugins link (keys, row ABIs, WGSL, data types) | yes |
//! | `class` | component classes others place or attach by name | no |
//! | `trait` | gameplay trait ids (`<plugin id>.<Trait>`) others implement or call | no |
//!
//! A *linked* kind is reached through a Cargo dependency, so the plugin
//! graph a descriptor declares and the one Cargo builds must agree
//! (`plugins/installed/tests/contracts.rs`); a runtime kind is reached by
//! name and needs no crate edge.
//!
//! How others do it: Unreal's `.uplugin` `Plugins` list and Unity's package
//! manifest name the plugin (package) depended on, with a version range, so
//! replacing a provider means editing every consumer; OSGi's
//! `Import-Package`/`Export-Package` name what is shared, and the resolver
//! picks a bundle that exports it. Ours is OSGi's shape with whole-plugin
//! enablement: consumers name contracts, a project picks which plugin
//! provides each by enabling it, and every failure names all three parties.
//! A contract has one integer `version` (no ranges): a consumer that pins a
//! version needs exactly that one, which is what a binary plugin's layout
//! fingerprint checks at load.

use std::fmt;

/// What kind of thing a contract shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractKind {
    /// Helio scene buffers (`members`: buffer keys).
    Buffer,
    /// Crates other plugins link (`members`: package names).
    Api,
    /// Component classes others place or attach (`members`: class names).
    Class,
    /// Gameplay traits (`members`: trait ids; empty means the contract id
    /// is the trait id).
    Trait,
}

impl ContractKind {
    /// Whether a consumer reaches it through a Cargo dependency on the
    /// provider (so the descriptor's graph must match Cargo's).
    pub fn linked(self) -> bool {
        matches!(self, Self::Buffer | Self::Api)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buffer => "buffer",
            Self::Api => "api",
            Self::Class => "class",
            Self::Trait => "trait",
        }
    }
}

impl fmt::Display for ContractKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn first_version() -> u32 {
    1
}

/// A contract a plugin provides (`provides = [{ id, kind, members, version }]`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidedContract {
    /// Stable id, namespaced by the provider's plugin id
    /// (`pulsar.lights.rows`).
    pub id: String,
    pub kind: ContractKind,
    /// What it covers, by kind ([`ContractKind`]).
    #[serde(default)]
    pub members: Vec<String>,
    /// Bumped when a change breaks consumers built against the old one.
    #[serde(default = "first_version")]
    pub version: u32,
}

impl ProvidedContract {
    /// Whether `member` (a buffer key, crate, class or trait id) is part of
    /// this contract. A trait contract with no members is its own id.
    pub fn covers(&self, member: &str) -> bool {
        self.members.iter().any(|m| m == member)
            || (self.kind == ContractKind::Trait && self.members.is_empty() && self.id == member)
    }
}

/// A contract a plugin needs (`requires = [{ id, optional, version }]`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredContract {
    pub id: String,
    /// The plugin works without it (a feature it adds when present). Its
    /// absence is recorded, not an error.
    #[serde(default)]
    pub optional: bool,
    /// The version the plugin was built against; `None` accepts any.
    #[serde(default)]
    pub version: Option<u32>,
}

/// Why resolution changed or refused part of a project's plugin list. Each
/// names every party: consumer, contract, provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The project enables a plugin this binary does not have.
    NotInstalled { plugin: String },
    /// A requirement no installed plugin provides.
    Unprovided { consumer: String, contract: String },
    /// A requirement whose providers are all off.
    ProviderDisabled {
        consumer: String,
        contract: String,
        provider: String,
        /// The project's list turns the provider off (rather than the
        /// provider being dropped for its own missing requirement).
        by_project: bool,
    },
    /// A requirement pinned to a version no provider has.
    Incompatible {
        consumer: String,
        contract: String,
        provider: String,
        provided: u32,
        required: u32,
    },
    /// Two enabled plugins provide one contract: the project must pick one
    /// by disabling the other.
    Conflict {
        contract: String,
        providers: Vec<String>,
    },
}

impl Problem {
    /// The plugin disabled by this problem, if any.
    pub fn consumer(&self) -> Option<&str> {
        match self {
            Self::Unprovided { consumer, .. }
            | Self::ProviderDisabled { consumer, .. }
            | Self::Incompatible { consumer, .. } => Some(consumer),
            Self::NotInstalled { .. } | Self::Conflict { .. } => None,
        }
    }

    /// Every plugin id the problem names.
    pub fn plugins(&self) -> Vec<&str> {
        match self {
            Self::NotInstalled { plugin } => vec![plugin],
            Self::Unprovided { consumer, .. } => vec![consumer],
            Self::ProviderDisabled {
                consumer, provider, ..
            }
            | Self::Incompatible {
                consumer, provider, ..
            } => vec![consumer, provider],
            Self::Conflict { providers, .. } => providers.iter().map(String::as_str).collect(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled { plugin } => {
                write!(
                    f,
                    "plugin {plugin} is enabled by the project but not installed"
                )
            }
            Self::Unprovided { consumer, contract } => write!(
                f,
                "plugin {consumer} needs contract {contract}, which no installed plugin provides; \
                 {consumer} is disabled"
            ),
            Self::ProviderDisabled {
                consumer,
                contract,
                provider,
                by_project,
            } => write!(
                f,
                "plugin {consumer} needs contract {contract}, provided by {provider}, which {}; \
                 {consumer} is disabled",
                if *by_project {
                    "the project disables"
                } else {
                    "is disabled"
                }
            ),
            Self::Incompatible {
                consumer,
                contract,
                provider,
                provided,
                required,
            } => write!(
                f,
                "plugin {consumer} needs contract {contract} version {required}, but {provider} \
                 provides version {provided}; {consumer} is disabled"
            ),
            Self::Conflict {
                contract,
                providers,
            } => write!(
                f,
                "contract {contract} is provided by {}; a project enables one provider of a \
                 contract",
                providers.join(" and ")
            ),
        }
    }
}

/// An optional requirement that nothing enabled provides: recorded so the
/// catalog can say what the consumer runs without.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbsentOptional {
    pub consumer: String,
    pub contract: String,
}
