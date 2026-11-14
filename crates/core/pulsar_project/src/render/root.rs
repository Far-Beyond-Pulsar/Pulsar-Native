//! The project's root `Cargo.toml`: a virtual workspace whose members are the
//! game crate and the scripts crates, the engine's dependency specs, patches
//! and profiles, every path relative and into the SDK mirror.
//!
//! What it carries from the engine is what applies to the game's graph:
//! a `[workspace.dependencies]` entry or a `[patch]` entry only for a crate
//! in that graph (or one the game crate names), a local one only when the
//! mirror has it; a profile's package overrides only for crates in the graph.

use std::collections::BTreeMap;

use super::{CLASS_DEPENDENCIES, GLUE_DEPENDENCIES, MANIFEST_HEADER};
use crate::model::ProjectModel;
use crate::paths;
use crate::sdk::SdkIndex;
use crate::toml_out::Doc;

pub(crate) struct RootManifest {
    members: Vec<String>,
    /// Name → spec (paths rebased into the mirror), sorted.
    pub(crate) dependencies: BTreeMap<String, toml::Value>,
    patches: BTreeMap<String, BTreeMap<String, toml::Value>>,
    profiles: toml::Table,
}

impl RootManifest {
    pub(crate) fn new(model: &ProjectModel, index: &SdkIndex) -> Self {
        let named_by_game: Vec<&str> = GLUE_DEPENDENCIES
            .iter()
            .chain(CLASS_DEPENDENCIES)
            .map(|(name, _)| *name)
            .chain(model.plugin_roots.iter().map(String::as_str))
            .collect();

        let mut dependencies = BTreeMap::new();
        for (name, spec) in &index.workspace.dependencies {
            if !index.crates.contains(name) && !named_by_game.contains(&name.as_str()) {
                continue;
            }
            if let Some(spec) = in_mirror(spec, index) {
                dependencies.insert(name.clone(), spec);
            }
        }

        let mut patches = BTreeMap::new();
        for (source, entries) in &index.workspace.patch {
            let Some(entries) = entries.as_table() else {
                continue;
            };
            let kept: BTreeMap<String, toml::Value> = entries
                .iter()
                .filter(|(name, _)| index.crates.contains(*name))
                .filter_map(|(name, spec)| Some((name.clone(), in_mirror(spec, index)?)))
                .collect();
            if !kept.is_empty() {
                patches.insert(source.clone(), kept);
            }
        }

        let mut members = vec![paths::GAME_DIR.to_owned()];
        members.extend(
            model
                .scripts
                .iter()
                .map(|s| format!("{}/{}", paths::SCRIPTS_DIR, s.dir)),
        );

        Self {
            members,
            dependencies,
            patches,
            profiles: profiles_for_graph(&index.workspace.profile, index),
        }
    }

    pub(crate) fn render(&self) -> String {
        let mut doc = Doc::new();
        doc.comment(MANIFEST_HEADER)
            .comment(
                "\nEngine crates are under .pulsar/sdk/, which each machine rebuilds\n\
                 (open the project in the editor, or run `pulsar project sync`).",
            )
            .section(&["workspace"])
            .str("resolver", "2")
            .entry("members", &strings(&self.members))
            .entry("exclude", &strings(&[paths::SDK_DIR.to_owned()]));

        doc.section(&["workspace", "dependencies"]);
        for (name, spec) in &self.dependencies {
            doc.entry(name, spec);
        }

        for (source, entries) in &self.patches {
            doc.section(&["patch", source]);
            for (name, spec) in entries {
                doc.entry(name, spec);
            }
        }

        for (profile, table) in &self.profiles {
            let Some(table) = table.as_table() else {
                continue;
            };
            doc.section(&["profile", profile]);
            for (key, value) in table.iter().filter(|(_, v)| !v.is_table()) {
                doc.entry(key, value);
            }
            for (key, value) in table.iter().filter(|(_, v)| v.is_table()) {
                doc.section(&["profile", profile, key]);
                for (inner, value) in value.as_table().into_iter().flatten() {
                    doc.entry(inner, value);
                }
            }
        }
        doc.finish()
    }
}

/// `spec` with its `path` (relative to the SDK root) rebased into the mirror,
/// or `None` when the mirror does not have that package. A spec with no path
/// is kept as it is.
fn in_mirror(spec: &toml::Value, index: &SdkIndex) -> Option<toml::Value> {
    let toml::Value::Table(table) = spec else {
        return Some(spec.clone());
    };
    let Some(path) = table.get("path").and_then(toml::Value::as_str) else {
        return Some(spec.clone());
    };
    let dir = paths::normalize_rel(path)?;
    if !index.has_dir(&dir) {
        return None;
    }
    let mut table = table.clone();
    table.insert(
        "path".into(),
        toml::Value::String(format!("{}/{dir}", paths::SDK_DIR)),
    );
    Some(toml::Value::Table(table))
}

/// The engine's profiles, each `package` override table kept only for
/// crates in the game's graph (and `"*"`), so Cargo has nothing to warn
/// about.
fn profiles_for_graph(profiles: &toml::Table, index: &SdkIndex) -> toml::Table {
    let mut out = profiles.clone();
    for (_, profile) in out.iter_mut() {
        let Some(profile) = profile.as_table_mut() else {
            continue;
        };
        if let Some(toml::Value::Table(packages)) = profile.get_mut("package") {
            packages.retain(|name, _| name == "*" || index.crates.contains(name));
            if packages.is_empty() {
                profile.remove("package");
            }
        }
    }
    out
}

fn strings(items: &[String]) -> toml::Value {
    toml::Value::Array(items.iter().cloned().map(toml::Value::String).collect())
}
