//! Plugin contracts (Pulsar-Native#1608, phase 3) over the installed set.
//!
//! - Every plugin's descriptor requires (optionally or not) a contract of
//!   exactly the plugins its crates link: the descriptor graph of linked
//!   contracts (`buffer`, `api`) equals the Cargo graph of normal
//!   dependencies between plugin directories, so the two cannot drift.
//! - An `api` contract names packages of its provider's directory.
//! - Every scene buffer a feature reads is linked to the plugin whose
//!   contract covers it (or to the frame), and the reader requires it.
//! - Activation names consumer, contract and provider for a missing, a
//!   disabled and an incompatible provider (fixture consumers in
//!   `tests/fixtures/contracts/`, resolved with the real installed set), and
//!   the editor's activation of a real project names the disabled provider.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use plugin_manager::{CatalogProblem, CatalogSnapshot, Provider};
use pulsar_plugins::{PluginDescriptor, Problem, ProjectPlugins};

use installed_plugins as _;

/// The tests change the process's activated plugins: one at a time.
static ACTIVATION: Mutex<()> = Mutex::new(());

fn vendor() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../vendor")
}

/// Directories inside a plugin that hold no crate of the plugin's own
/// (examples and demos build against it; fixtures are tests').
const NOT_THE_PLUGINS: &[&str] = &[
    "examples",
    "fixtures",
    "web",
    "android",
    "web-demos",
    "android-demos",
    "target",
    "node_modules",
];

/// Every plugin directory with a descriptor: (dir, descriptor, its packages).
fn plugins() -> Vec<(PathBuf, PluginDescriptor, Vec<PathBuf>)> {
    let mut plugins = Vec::new();
    for entry in std::fs::read_dir(vendor()).unwrap() {
        let dir = entry.unwrap().path();
        let Ok(source) = std::fs::read_to_string(dir.join(pulsar_plugins::DESCRIPTOR_FILE)) else {
            continue;
        };
        let descriptor = PluginDescriptor::parse(&source)
            .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
        let mut manifests = Vec::new();
        collect_manifests(&dir, &mut manifests);
        plugins.push((dir, descriptor, manifests));
    }
    plugins.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    plugins
}

fn collect_manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    let manifest = dir.join("Cargo.toml");
    if manifest.is_file() {
        out.push(manifest);
    }
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() && !name.starts_with('.') && !NOT_THE_PLUGINS.contains(&name.as_str()) {
            collect_manifests(&path, out);
        }
    }
}

fn package_name(manifest: &toml::Table) -> Option<String> {
    manifest
        .get("package")?
        .get("name")?
        .as_str()
        .map(|name| name.replace('-', "_"))
}

/// The packages a manifest depends on (normal dependencies), by package
/// name (a renamed dependency by its `package`).
fn normal_dependencies(manifest: &toml::Table) -> Vec<String> {
    let Some(deps) = manifest.get("dependencies").and_then(|d| d.as_table()) else {
        return Vec::new();
    };
    deps.iter()
        .map(|(key, value)| {
            value
                .get("package")
                .and_then(|p| p.as_str())
                .unwrap_or(key)
                .replace('-', "_")
        })
        .collect()
}

#[test]
fn descriptors_require_what_their_crates_link() {
    let plugins = plugins();
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut manifests: Vec<(String, toml::Table)> = Vec::new();
    for (_, descriptor, paths) in &plugins {
        for path in paths {
            let table: toml::Table = std::fs::read_to_string(path).unwrap().parse().unwrap();
            if let Some(name) = package_name(&table) {
                owner.insert(name, descriptor.id.clone());
                manifests.push((descriptor.id.clone(), table));
            }
        }
    }
    let mut linked: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (plugin, table) in &manifests {
        for dependency in normal_dependencies(table) {
            if let Some(provider) = owner.get(&dependency) {
                if provider != plugin {
                    linked
                        .entry(plugin.clone())
                        .or_default()
                        .insert(provider.clone());
                }
            }
        }
    }
    let provider_of = |contract: &str| {
        plugins
            .iter()
            .find(|(_, d, _)| d.provided(contract).is_some())
            .map(|(_, d, _)| (d.id.clone(), d.provided(contract).unwrap().kind))
    };
    let mut wrong = Vec::new();
    for (_, descriptor, _) in &plugins {
        let mut declared = BTreeSet::new();
        for required in &descriptor.requires {
            match provider_of(&required.id) {
                Some((provider, kind)) if kind.linked() => {
                    declared.insert(provider);
                }
                Some(_) => {}
                None => wrong.push(format!(
                    "{} requires {}, which no installed plugin provides",
                    descriptor.id, required.id
                )),
            }
        }
        let cargo = linked.remove(&descriptor.id).unwrap_or_default();
        for missing in cargo.difference(&declared) {
            wrong.push(format!(
                "{}'s crates link {missing}'s, but its descriptor requires no buffer or api \
                 contract of {missing}",
                descriptor.id
            ));
        }
        for stale in declared.difference(&cargo) {
            wrong.push(format!(
                "{} requires a linked contract of {stale}, but none of its crates depends on \
                 {stale}'s",
                descriptor.id
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_api_contract_names_its_providers_packages() {
    let mut wrong = Vec::new();
    for (_, descriptor, paths) in plugins() {
        let packages: BTreeSet<String> = paths
            .iter()
            .filter_map(|path| {
                package_name(&std::fs::read_to_string(path).unwrap().parse().unwrap())
            })
            .collect();
        for contract in &descriptor.provides {
            if contract.kind != pulsar_plugins::ContractKind::Api {
                continue;
            }
            for member in &contract.members {
                if !packages.contains(&member.replace('-', "_")) {
                    wrong.push(format!(
                        "{}'s contract {} names `{member}`, which is not one of its packages",
                        descriptor.id, contract.id
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn every_feature_read_links_to_a_provider_its_plugin_requires() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let catalog = CatalogSnapshot::snapshot();
    let contract_problems: Vec<String> = catalog
        .problems
        .iter()
        .filter(|problem| {
            matches!(
                problem,
                CatalogProblem::UnprovidedBuffer { .. } | CatalogProblem::UndeclaredRead { .. }
            )
        })
        .map(ToString::to_string)
        .collect();
    assert!(
        contract_problems.is_empty(),
        "{}",
        contract_problems.join("\n")
    );
    // The reads are there to link: the plugins that draw declare theirs.
    for (plugin, key, provider) in [
        ("pulsar.fog", "scene_lights", "pulsar.lights"),
        ("pulsar.meshes", "materials", "pulsar.materials"),
        ("pulsar.water", "water_volumes", "pulsar.water"),
    ] {
        let reads = &catalog.plugin(plugin).unwrap().reads;
        assert!(
            reads.iter().any(|link| link.key == key
                && matches!(link.provider, Provider::Plugin { plugin, .. } if plugin == provider)),
            "{plugin} reads {key} from {provider}: {reads:?}"
        );
    }
}

/// A fixture consumer's descriptor (`tests/fixtures/contracts/<name>.toml`).
fn fixture(name: &str) -> PluginDescriptor {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/contracts")
        .join(format!("{name}.toml"));
    PluginDescriptor::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn resolve_with(consumer: PluginDescriptor, project: &ProjectPlugins) -> Vec<Problem> {
    let mut installed = pulsar_plugins::installed().to_vec();
    let id = consumer.id.clone();
    installed.push(consumer);
    let resolution = pulsar_plugins::resolve(&installed, project);
    assert!(!resolution.is_enabled(&id), "{id} is disabled");
    resolution
        .problems
        .into_iter()
        .filter(|problem| problem.consumer() == Some(id.as_str()))
        .collect()
}

#[test]
fn a_missing_a_disabled_and_an_incompatible_provider_are_named() {
    let missing = resolve_with(fixture("missing_provider"), &ProjectPlugins::default());
    assert_eq!(
        missing,
        [Problem::Unprovided {
            consumer: "pulsar.test_missing".into(),
            contract: "pulsar.nowhere.rows".into()
        }]
    );
    assert_eq!(
        missing[0].to_string(),
        "plugin pulsar.test_missing needs contract pulsar.nowhere.rows, which no installed \
         plugin provides; pulsar.test_missing is disabled"
    );

    let disabled = resolve_with(
        fixture("disabled_provider"),
        &ProjectPlugins::default().with(&["pulsar.lights"], false),
    );
    assert_eq!(
        disabled[0].to_string(),
        "plugin pulsar.test_disabled needs contract pulsar.lights.rows, provided by \
         pulsar.lights, which the project disables; pulsar.test_disabled is disabled"
    );

    let incompatible = resolve_with(fixture("incompatible_provider"), &ProjectPlugins::default());
    assert_eq!(
        incompatible[0].to_string(),
        "plugin pulsar.test_incompatible needs contract pulsar.lights.rows version 2, but \
         pulsar.lights provides version 1; pulsar.test_incompatible is disabled"
    );
}

/// The editor's own path: opening a project that disables the meshes
/// disables the asset viewer, which requires their api, and the catalog
/// shows the named error on both.
#[test]
fn opening_a_project_names_a_disabled_provider() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    let project = tempfile::tempdir().unwrap();
    let resolution = pulsar_plugins::activate_project(
        project.path(),
        Ok(ProjectPlugins::default().with(&["pulsar.meshes"], false)),
    );
    let catalog = CatalogSnapshot::snapshot();
    pulsar_plugins::deactivate();
    assert!(!resolution.is_enabled("pulsar.asset_viewer"));
    let named = "plugin pulsar.asset_viewer needs contract pulsar.meshes.api, provided by \
                 pulsar.meshes, which the project disables; pulsar.asset_viewer is disabled";
    assert!(
        resolution.problems.iter().any(|p| p.to_string() == named),
        "{:?}",
        resolution.problems
    );
    let viewer = catalog.plugin("pulsar.asset_viewer").unwrap();
    assert!(!viewer.enabled && viewer.has_unmet_requirement());
    assert!(catalog
        .activation_problems()
        .iter()
        .any(|p| p.to_string() == named));
    // The plugins that only optionally read meshes stay.
    assert!(resolution.is_enabled("pulsar.gi"));
    assert!(resolution
        .absent_optional
        .iter()
        .any(|absent| absent.consumer == "pulsar.gi" && absent.contract == "pulsar.meshes.rows"));
}
