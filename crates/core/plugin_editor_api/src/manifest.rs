//! Plugin manifest: identity and the plugins a plugin builds on.
//!
//! A plugin names the plugins it needs in [`PluginManifest::requires`]. The
//! host reads every manifest (`_plugin_manifest`, exported by
//! `export_plugin!`) before it runs any plugin's constructor, loads plugins
//! in dependency order, and refuses a plugin whose dependencies are missing,
//! too old, cyclic or failed to load.

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::identifiers::PluginId;
use crate::metadata::PluginMetadata;

/// A plugin another plugin needs, with the versions it accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDependency {
    pub id: PluginId,
    pub requirement: VersionReq,
}

impl PluginDependency {
    /// `requirement` uses cargo's syntax (`"^0.1"`, `">=1.2, <2"`, `"*"`).
    ///
    /// # Panics
    ///
    /// On a requirement that does not parse. A manifest is written by the
    /// plugin's author, so a bad one is a bug in the plugin; use
    /// [`PluginDependency::parse`] to handle it instead.
    pub fn new(id: impl Into<String>, requirement: &str) -> Self {
        Self::parse(id, requirement)
            .unwrap_or_else(|e| panic!("invalid plugin version requirement {requirement:?}: {e}"))
    }

    pub fn parse(
        id: impl Into<String>,
        requirement: &str,
    ) -> Result<Self, semver::Error> {
        Ok(Self {
            id: PluginId::new(id),
            requirement: VersionReq::parse(requirement)?,
        })
    }

    /// Any version of `id`.
    pub fn any(id: impl Into<String>) -> Self {
        Self {
            id: PluginId::new(id),
            requirement: VersionReq::STAR,
        }
    }
}

/// What the host needs to know about a plugin before it runs any of the
/// plugin's code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    /// Plugins this one cannot work without. They load first; if one is
    /// absent or fails, this plugin does not load.
    pub requires: Vec<PluginDependency>,
    /// Plugins this one uses when present. They load first when they are
    /// there, and their absence is not an error.
    pub optional: Vec<PluginDependency>,
}

impl PluginManifest {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        author: impl Into<String>,
    ) -> Self {
        Self {
            id: PluginId::new(id),
            name: name.into(),
            version: version.into(),
            author: author.into(),
            description: String::new(),
            requires: Vec::new(),
            optional: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn requires(mut self, dependency: PluginDependency) -> Self {
        self.requires.push(dependency);
        self
    }

    pub fn optional(mut self, dependency: PluginDependency) -> Self {
        self.optional.push(dependency);
        self
    }

    /// The manifest of a plugin that declares no dependencies.
    pub fn from_metadata(metadata: &PluginMetadata) -> Self {
        Self {
            id: metadata.id.clone(),
            name: metadata.name.clone(),
            version: metadata.version.clone(),
            author: metadata.author.clone(),
            description: metadata.description.clone(),
            requires: Vec::new(),
            optional: Vec::new(),
        }
    }

    pub fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            author: self.author.clone(),
            description: self.description.clone(),
        }
    }

    /// `version` as semver, if it is.
    pub fn parsed_version(&self) -> Option<Version> {
        Version::parse(&self.version).ok()
    }
}

/// Whether `manifest` satisfies `dependency`. A version that is not semver
/// satisfies only a requirement of `*`.
pub fn satisfies(manifest: &PluginManifest, dependency: &PluginDependency) -> bool {
    if manifest.id != dependency.id {
        return false;
    }
    match manifest.parsed_version() {
        Some(version) => dependency.requirement.matches(&version),
        None => dependency.requirement == VersionReq::STAR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, version: &str) -> PluginManifest {
        PluginManifest::new(id, id, version, "test")
    }

    #[test]
    fn caret_requirement_matches_compatible_versions_only() {
        let dep = PluginDependency::new("a", "^0.1");
        assert!(satisfies(&manifest("a", "0.1.4"), &dep));
        assert!(!satisfies(&manifest("a", "0.2.0"), &dep));
        assert!(!satisfies(&manifest("b", "0.1.0"), &dep));
    }

    #[test]
    fn unparsable_version_satisfies_only_any() {
        let m = manifest("a", "nightly");
        assert!(satisfies(&m, &PluginDependency::any("a")));
        assert!(!satisfies(&m, &PluginDependency::new("a", "^1")));
    }

    #[test]
    fn metadata_round_trips() {
        let m = manifest("a", "1.0.0").description("d");
        assert_eq!(PluginManifest::from_metadata(&m.metadata()), m);
    }
}
