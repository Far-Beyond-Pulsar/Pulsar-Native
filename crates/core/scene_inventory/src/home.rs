//! Which ledger a row belongs in (Pulsar-Native#1606): a plugin owns the rows
//! of what it ships (its classes, schemas, buffers, pass crates and call
//! sites), in `plugins/vendor/<dir>/ledger.toml`; the engine's rows stay in
//! this crate's `ledger.toml`. A row's home is computed from the facts, not
//! from its human columns: a class's registering root, a schema's crate, a
//! buffer's producers, a pass crate's directory, a site's file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::linked::LinkedClass;
use crate::source::{BufferGraph, PassCrate};

/// The ledger a row belongs in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Home {
    /// The engine's: `crates/core/scene_inventory/ledger.toml`.
    Core,
    /// An installed plugin's, by its directory under `plugins/vendor`.
    Plugin(String),
}

impl Home {
    /// The repository-relative ledger file.
    pub fn ledger_file(&self) -> String {
        match self {
            Home::Core => CORE_LEDGER.to_string(),
            Home::Plugin(dir) => format!("plugins/vendor/{dir}/ledger.toml"),
        }
    }
}

/// The engine's ledger, repository-relative.
pub const CORE_LEDGER: &str = "crates/core/scene_inventory/ledger.toml";

// std::fs allowed: the inventory reads the engine's own source checkout (a CI/dev check), not a project
#[allow(clippy::disallowed_methods)]
/// Every installed plugin's directory under `plugins/vendor` (the
/// directories with a `pulsar-plugin.toml`), sorted.
pub fn plugin_dirs(root: &Path) -> Vec<String> {
    let mut dirs: Vec<String> = std::fs::read_dir(root.join("plugins/vendor"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("pulsar-plugin.toml").is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    dirs.sort();
    dirs
}

// std::fs allowed: the inventory reads the engine's own source checkout (a CI/dev check), not a project
#[allow(clippy::disallowed_methods)]
/// Every ledger file: the engine's, then each installed plugin's that exists.
pub fn ledger_files(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![root.join(CORE_LEDGER)];
    files.extend(
        plugin_dirs(root)
            .into_iter()
            .map(|dir| root.join(Home::Plugin(dir).ledger_file()))
            .filter(|path| path.is_file()),
    );
    files
}

/// The homes of packages and paths in this checkout.
pub struct Homes {
    plugins: BTreeSet<String>,
    /// Package name (and its crate name, `-` as `_`) -> repository-relative
    /// directory.
    package_dirs: HashMap<String, String>,
}

impl Homes {
    pub fn new(root: &Path) -> Self {
        Self {
            plugins: plugin_dirs(root).into_iter().collect(),
            package_dirs: package_dirs(root),
        }
    }

    /// The home of a repository-relative file or directory.
    pub fn of_path(&self, rel: &str) -> Home {
        rel.strip_prefix("plugins/vendor/")
            .and_then(|rest| rest.split('/').next())
            .filter(|dir| self.plugins.contains(*dir))
            .map_or(Home::Core, |dir| Home::Plugin(dir.to_string()))
    }

    /// The home of a package or crate name.
    pub fn of_package(&self, package: &str) -> Home {
        self.package_dirs
            .get(package)
            .map_or(Home::Core, |dir| self.of_path(dir))
    }

    /// A class's: the plugin whose root registers it.
    pub fn class(&self, class: &LinkedClass) -> Home {
        class
            .origin
            .as_deref()
            .filter(|origin| !origin.starts_with("engine:"))
            .map_or(Home::Core, |origin| self.of_package(origin))
    }

    /// A schema's: its type's crate (`lights_component::...`).
    pub fn schema(&self, path: &str) -> Home {
        self.of_package(path.split("::").next().unwrap_or(path))
    }

    /// A buffer's: the one plugin its producers (else its consumers) are in;
    /// a buffer the engine or several plugins produce is the engine's.
    pub fn buffer(&self, key: &str, graph: &BufferGraph) -> Home {
        let of = |packages: Option<&BTreeSet<String>>| -> BTreeSet<Home> {
            packages
                .into_iter()
                .flatten()
                .map(|package| self.of_package(package))
                .collect()
        };
        let mut homes = of(graph.producer_packages.get(key));
        if homes.is_empty() {
            homes = of(graph.consumers.get(key));
        }
        match homes.len() {
            1 => homes.into_iter().next().unwrap(),
            _ => Home::Core,
        }
    }

    /// A pass crate's: its directory's.
    pub fn pass(&self, pass: &PassCrate) -> Home {
        self.of_path(&pass.dir)
    }
}

/// Package name (and crate name) -> repository-relative directory, from
/// `cargo metadata` (workspace members and path dependencies).
fn package_dirs(root: &Path) -> HashMap<String, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .current_dir(root)
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("metadata JSON");
    // Cargo's own root, not `root`: a canonicalized Windows path (`\\?\C:`)
    // never prefixes cargo's manifest paths.
    let root = metadata["workspace_root"]
        .as_str()
        .expect("metadata has a workspace root")
        .replace('\\', "/");
    let mut dirs = HashMap::new();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        let (Some(name), Some(manifest)) =
            (package["name"].as_str(), package["manifest_path"].as_str())
        else {
            continue;
        };
        let manifest = manifest.replace('\\', "/");
        let Some(rel) = manifest
            .strip_prefix(&format!("{root}/"))
            .and_then(|m| m.strip_suffix("/Cargo.toml"))
        else {
            continue;
        };
        dirs.insert(name.to_string(), rel.to_string());
        dirs.insert(name.replace('-', "_"), rel.to_string());
    }
    dirs
}

/// `(row, file)` for every row that is not in its home's ledger.
pub fn misplaced(
    placed: &BTreeMap<String, String>,
    homes: &BTreeMap<String, Home>,
) -> Vec<(String, String)> {
    homes
        .iter()
        .filter_map(|(row, home)| {
            let file = placed.get(row)?;
            (file != &home.ledger_file()).then(|| (row.clone(), home.ledger_file()))
        })
        .collect()
}
