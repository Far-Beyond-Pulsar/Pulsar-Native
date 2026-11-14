//! The SDK index: everything a game project needs to know about the engine
//! it builds against, with every path relative to the SDK root.
//!
//! - **packages**: the engine's local packages a game's dependency graph
//!   activates, each with its directory. Computed by Cargo itself
//!   (`cargo tree`, normal and build edges, every target) from the crates a
//!   game links: the game crate's engine dependencies and every runtime
//!   plugin's root crate. Inactive optional dependencies are left out: Cargo
//!   reads no manifest of an inactive optional path dependency of a package
//!   that is not a workspace member, which every mirrored package is.
//! - **crates**: the name of every package in that graph, local or not
//!   (which `[patch]` entries and profile overrides apply to a game).
//! - **workspace**: the engine workspace's `[workspace.package]`,
//!   `[workspace.dependencies]`, `[patch]` and `[profile]` tables, which
//!   normalize mirrored manifests and seed the game's root manifest.
//! - **plugins**: the descriptors of the engine's plugins.
//! - **lock**: the engine's `Cargo.lock`, which seeds the game's.
//!
//! Built from a source checkout ([`SdkIndex::build`]); an exported SDK ships
//! it as [`INDEX_FILE`].

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::fs::{is_file, read_string as read};
use crate::hash;

/// Bumped when the index's meaning changes; an index of another format is
/// rebuilt (checkout) or refused (exported SDK).
pub const INDEX_FORMAT: u32 = 1;

/// The index file at the root of an exported SDK.
pub const INDEX_FILE: &str = "pulsar-sdk.json";

/// The engine crates a generated game crate depends on by name, beyond the
/// plugin roots: its glue and the Rust it generates for classes. Those that
/// are members of the engine workspace root the dependency graph.
pub const GAME_ENGINE_CRATES: &[&str] = &[
    "pulsar_game",
    "pulsar_pie_abi",
    "engine_class_derive",
    "pulsar_world_registry",
    "pulsar_scenedb",
    "pulsar_reflection",
    "pulsar_script_vm",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SdkIndex {
    pub format: u32,
    /// Sorted by `dir`.
    pub packages: Vec<SdkPackage>,
    pub crates: BTreeSet<String>,
    pub workspace: SdkWorkspace,
    /// Descriptor sources (`pulsar-plugin.toml`), sorted by plugin id.
    pub plugins: Vec<String>,
    pub lock: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkPackage {
    pub name: String,
    pub version: String,
    /// Relative to the SDK root, forward slashes.
    pub dir: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SdkWorkspace {
    pub package: toml::Table,
    pub dependencies: toml::Table,
    pub patch: toml::Table,
    pub profile: toml::Table,
}

impl SdkIndex {
    /// Identifies this SDK: a change to anything a game is generated from
    /// changes it.
    pub fn fingerprint(&self) -> String {
        let json = serde_json::to_vec(self).expect("the index serializes");
        hash::sha256_hex(&json)
    }

    pub fn package_by_name(&self, name: &str) -> Option<&SdkPackage> {
        self.packages.iter().find(|p| p.name == name)
    }

    pub fn has_dir(&self, dir: &str) -> bool {
        self.packages.iter().any(|p| p.dir == dir)
    }

    /// The parsed plugin descriptors, sorted by id.
    pub fn plugin_descriptors(&self) -> Result<Vec<pulsar_plugins::PluginDescriptor>, String> {
        self.plugins
            .iter()
            .map(|source| pulsar_plugins::PluginDescriptor::parse(source))
            .collect()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the index serializes")
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let index: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if index.format != INDEX_FORMAT {
            return Err(format!(
                "SDK index format {} (this engine reads format {INDEX_FORMAT})",
                index.format
            ));
        }
        Ok(index)
    }

    /// Build the index of the engine source checkout at `root` (the
    /// directory of its workspace `Cargo.toml`). Runs Cargo there with
    /// `--locked`: the checkout is only read.
    pub fn build(root: &Path) -> Result<Self, String> {
        let manifest_path = root.join("Cargo.toml");
        let manifest_text = read(&manifest_path)?;
        let manifest: toml::Table = toml::from_str(&manifest_text)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        let workspace = workspace_tables(&manifest)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;

        let members = workspace_members(root)?;
        let mut plugins: Vec<(String, String)> = Vec::new();
        for member in &members {
            let descriptor = member.dir.join(pulsar_plugins::DESCRIPTOR_FILE);
            if !is_file(&descriptor) {
                continue;
            }
            let source = read(&descriptor)?;
            let parsed = pulsar_plugins::PluginDescriptor::parse(&source)
                .map_err(|e| format!("{}: {e}", descriptor.display()))?;
            if parsed.root != member.name {
                return Err(format!(
                    "{}: `root = \"{}\"` but the package beside it is `{}`",
                    descriptor.display(),
                    parsed.root,
                    member.name
                ));
            }
            plugins.push((parsed.id.clone(), source));
        }
        plugins.sort();

        let member_names: BTreeSet<&str> = members.iter().map(|m| m.name.as_str()).collect();
        let mut roots: BTreeSet<String> = GAME_ENGINE_CRATES
            .iter()
            .filter(|name| member_names.contains(*name))
            .map(|name| (*name).to_owned())
            .collect();
        for (_, source) in &plugins {
            let descriptor = pulsar_plugins::PluginDescriptor::parse(source)?;
            if descriptor.runtime() {
                roots.insert(descriptor.root);
            }
        }
        if !roots.contains("pulsar_game") {
            return Err(format!(
                "{} has no `pulsar_game` package: not an engine checkout",
                root.display()
            ));
        }

        let tree = cargo_tree(root, &roots)?;
        let (mut packages, crates) = parse_tree(&tree, root)?;
        packages.sort_by(|a, b| a.dir.cmp(&b.dir));
        packages.dedup_by(|a, b| a.dir == b.dir);

        let lock = read(&root.join("Cargo.lock"))?;
        Ok(Self {
            format: INDEX_FORMAT,
            packages,
            crates,
            workspace,
            plugins: plugins.into_iter().map(|(_, source)| source).collect(),
            lock,
        })
    }
}

/// The workspace tables an index carries, from an engine's root manifest.
pub(crate) fn workspace_tables(manifest: &toml::Table) -> Result<SdkWorkspace, String> {
    let table = |value: Option<&toml::Value>, what: &str| -> Result<toml::Table, String> {
        match value {
            None => Ok(toml::Table::new()),
            Some(toml::Value::Table(t)) => Ok(t.clone()),
            Some(_) => Err(format!("`{what}` is not a table")),
        }
    };
    let ws = manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .ok_or("no [workspace] table: not a workspace root")?;
    Ok(SdkWorkspace {
        package: table(ws.get("package"), "workspace.package")?,
        dependencies: table(ws.get("dependencies"), "workspace.dependencies")?,
        patch: table(manifest.get("patch"), "patch")?,
        profile: table(manifest.get("profile"), "profile")?,
    })
}

struct Member {
    name: String,
    dir: std::path::PathBuf,
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// The engine workspace's members (`cargo metadata --no-deps`).
fn workspace_members(root: &Path) -> Result<Vec<Member>, String> {
    let output = cargo()
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("running cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata in {} failed:\n{}",
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    #[derive(Deserialize)]
    struct Metadata {
        packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
        manifest_path: std::path::PathBuf,
    }
    let metadata: Metadata = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("reading cargo metadata: {e}"))?;
    let mut members: Vec<Member> = metadata
        .packages
        .into_iter()
        .filter_map(|p| {
            Some(Member {
                dir: p.manifest_path.parent()?.to_path_buf(),
                name: p.name,
            })
        })
        .collect();
    members.sort_by(|a, b| a.dir.cmp(&b.dir));
    Ok(members)
}

/// `cargo tree` from `roots`: one `name vVERSION (source)` line per package
/// reached through normal and build edges on any target.
fn cargo_tree(root: &Path, roots: &BTreeSet<String>) -> Result<String, String> {
    let mut command = cargo();
    command
        .args(["tree", "--locked", "-e", "normal,build", "--target", "all"])
        .args(["--prefix", "none", "--format", "{p}"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"));
    for name in roots {
        command.arg("-p").arg(name);
    }
    let output = command
        .output()
        .map_err(|e| format!("running cargo tree: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo tree in {} failed:\n{}",
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("cargo tree output: {e}"))
}

/// The local packages (under `root`) and every package name in a
/// `cargo tree --format {p}` listing.
fn parse_tree(tree: &str, root: &Path) -> Result<(Vec<SdkPackage>, BTreeSet<String>), String> {
    let mut packages = Vec::new();
    let mut crates = BTreeSet::new();
    for line in tree.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some(package) = parse_tree_line(line) else {
            return Err(format!("unexpected cargo tree line: {line}"));
        };
        crates.insert(package.name.to_owned());
        let Some(source) = package.source else {
            continue;
        };
        if !Path::new(source).is_absolute() {
            continue; // a git source
        }
        let rel = strip_root(source, &root.to_string_lossy()).ok_or_else(|| {
                format!(
                    "package `{}` is at {source}, outside the engine checkout {}: an SDK must hold every local package",
                    package.name,
                    root.display()
                )
            })?;
        packages.push(SdkPackage {
            name: package.name.to_owned(),
            version: package.version.to_owned(),
            dir: rel,
        });
    }
    Ok((packages, crates))
}

struct TreeLine<'a> {
    name: &'a str,
    version: &'a str,
    source: Option<&'a str>,
}

/// `name vX.Y.Z`, then optionally ` (source)`, ` (proc-macro)` and ` (*)`.
fn parse_tree_line(line: &str) -> Option<TreeLine<'_>> {
    let mut rest = line;
    loop {
        let before = rest;
        for suffix in [" (*)", " (proc-macro)"] {
            rest = rest.strip_suffix(suffix).unwrap_or(rest);
        }
        if rest == before {
            break;
        }
    }
    let (name, rest) = rest.split_once(' ')?;
    let (version, rest) = match rest.split_once(' ') {
        Some((version, rest)) => (version, Some(rest)),
        None => (rest, None),
    };
    let version = version.strip_prefix('v')?;
    let source = match rest {
        None => None,
        Some(rest) => Some(rest.strip_prefix('(')?.strip_suffix(')')?),
    };
    Some(TreeLine {
        name,
        version,
        source,
    })
}

/// `path` relative to `root`, forward slashes, or `None` when it is not
/// inside it. Windows paths compare ASCII-case-insensitively: Cargo and
/// `current_exe` can spell a drive letter or folder in different case.
pub(crate) fn strip_root(path: &str, root: &str) -> Option<String> {
    fn clean(path: &str) -> String {
        let path = path
            .strip_prefix(r"\\?\")
            .or_else(|| path.strip_prefix("//?/"))
            .unwrap_or(path);
        path.replace('\\', "/").trim_end_matches('/').to_owned()
    }
    let (path, root) = (clean(path), clean(root));
    if path.len() < root.len() || !path.is_char_boundary(root.len()) {
        return None;
    }
    let (head, rest) = path.split_at(root.len());
    let same = if cfg!(windows) {
        head.eq_ignore_ascii_case(&root)
    } else {
        head == root
    };
    (same && (rest.is_empty() || rest.starts_with('/')))
        .then(|| rest.trim_start_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_lines_parse_with_their_markers() {
        let line = parse_tree_line(r"pulsar_game v0.1.0 (D:\e\crates\core\pulsar_game) (*)").unwrap();
        assert_eq!(
            (line.name, line.version, line.source),
            ("pulsar_game", "0.1.0", Some(r"D:\e\crates\core\pulsar_game"))
        );
        let line = parse_tree_line("serde_derive v1.0.228 (proc-macro)").unwrap();
        assert_eq!((line.name, line.source), ("serde_derive", None));
        let line = parse_tree_line(
            "wgpu v30.0.1 (https://github.com/x/wgpu?branch=helio%2Fv30.0.1#abc) (*)",
        )
        .unwrap();
        assert_eq!(line.version, "30.0.1");
        assert!(line.source.unwrap().starts_with("https://"));
        let line =
            parse_tree_line(r"m v0.1.0 (C:\Program Files (x86)\e\m) (proc-macro) (*)").unwrap();
        assert_eq!(line.source, Some(r"C:\Program Files (x86)\e\m"));
    }

    #[test]
    fn local_packages_are_relative_to_the_root_and_outsiders_are_refused() {
        let root = if cfg!(windows) {
            Path::new(r"D:\Engine")
        } else {
            Path::new("/engine")
        };
        let inside = if cfg!(windows) {
            r"d:\engine\crates\core\pulsar_game"
        } else {
            "/engine/crates/core/pulsar_game"
        };
        let tree = format!("pulsar_game v0.1.0 ({inside})\nserde v1.0.0\n");
        let (packages, crates) = parse_tree(&tree, root).unwrap();
        assert_eq!(packages[0].dir, "crates/core/pulsar_game");
        assert!(crates.contains("serde"));

        let outside = if cfg!(windows) {
            r"D:\EngineOther\x"
        } else {
            "/engineother/x"
        };
        let tree = format!("x v0.1.0 ({outside})\n");
        assert!(parse_tree(&tree, root).is_err());
    }
}
