//! Generates `tools/workspace_hack`: one pinned feature set for every crate inside the shared libraries.
//!
//! Cargo unifies a crate's features across everything in one build, and rustc treats a crate built
//! with other features as another crate. So a build of the editor alone and a build of the editor with
//! every plugin yield different shared libraries, and a library released from one cannot serve the
//! other. `pulsar_workspace_hack` depends on every crate inside the shared libraries with the union of
//! the features the editor and every plugin ask of it; the editor, and any build of a plugin that adds
//! `-p pulsar_workspace_hack`, then get the same crates whatever else is in the build.
//!
//! The union is read from cargo's own resolver (`cargo tree --target all`), for the build
//! `tools/plugin_builder` does: the editor, every plugin root with its `dll` feature, and the shared
//! libraries. Run `cargo run -p hack_gen` after a dependency or feature change; CI runs it with
//! `--check`.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use engine_fs::virtual_fs;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The shared libraries (one per owner repository); the crates inside them are what gets pinned.
const SHARED_LIBRARIES: &[&str] = &[
    "wgpui_dylib",
    "ui_dylib",
    "profiling_dylib",
    "reflection_dylib",
    "scenedb_dylib",
    "helio_dylib",
];

/// Plugins the editor links rather than loads: no library, no `dll` feature.
const LINKED_ROOTS: &[&str] = &["render_bps_plugin"];

/// A package as `cargo tree` prints it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Package {
    name: String,
    version: String,
    /// `None` for a registry crate; a git URL with its reference, or a path.
    source: Option<String>,
}

fn repo_root() -> PathBuf {
    // tools/hack_gen -> the repository root, without `..` or a verbatim prefix: cargo prints plain paths.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().and_then(Path::parent).unwrap_or(manifest).to_path_buf()
}

/// The roots of the plugins that build as libraries.
fn plugin_roots(root: &Path) -> Result<Vec<String>> {
    let vendor = root.join("plugins").join("vendor");
    let mut roots = Vec::new();
    for entry in virtual_fs::list_dir(&vendor)? {
        let descriptor = vendor.join(&entry.name).join("pulsar-plugin.toml");
        if !entry.is_dir || !virtual_fs::is_file(&descriptor) {
            continue;
        }
        let text = virtual_fs::read_to_string(&descriptor)?;
        if let Some(name) = text
            .lines()
            .find_map(|line| line.strip_prefix("root = \"")?.strip_suffix('"'))
        {
            if !LINKED_ROOTS.contains(&name) {
                roots.push(name.to_owned());
            }
        }
    }
    roots.sort();
    Ok(roots)
}

/// One line of `cargo tree --prefix none -f "{p}|{f}"`: the package and its enabled features.
fn parse_line(line: &str) -> Option<(Package, Vec<String>)> {
    let line = line.trim().trim_end_matches(" (*)");
    let (package, features) = line.rsplit_once('|')?;
    let mut parts = package.splitn(3, ' ');
    let name = parts.next()?.to_owned();
    let version = parts.next()?.strip_prefix('v')?.to_owned();
    let mut source = None;
    if let Some(rest) = parts.next() {
        for group in rest.split(") (") {
            let group = group.trim_matches(|c| c == '(' || c == ')');
            if !group.is_empty() && group != "proc-macro" {
                source = Some(group.to_owned());
            }
        }
    }
    let features = features
        .split(',')
        .filter(|feature| !feature.is_empty())
        .map(str::to_owned)
        .collect();
    Some((Package { name, version, source }, features))
}

/// The platforms the engine and its plugins are released for. A dependency only some of them need goes in that
/// platform's own table: the hack must not make Windows compile `objc2`.
const TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "i686-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
];

fn cargo_tree(root: &Path, target: &str, args: &[String]) -> Result<Vec<(Package, Vec<String>)>> {
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root)
        .args(["tree", "--target", target, "-e", "normal,build", "--prefix", "none"])
        .args(["-f", "{p}|{f}"])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!("cargo tree failed:\n{}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter_map(parse_line)
        .collect())
}

/// The dependency specification of `package` with its pinned features.
fn dependency(package: &Package, features: &BTreeSet<String>, root: &Path, renamed: bool) -> String {
    let features: Vec<String> = features.iter().map(|f| format!("\"{f}\"")).collect();
    let rename = if renamed { format!("package = \"{}\", ", package.name) } else { String::new() };
    let features = format!("default-features = false, features = [{}]", features.join(", "));
    match package.source.as_deref() {
        None => format!("{{ {rename}version = \"={}\", {features} }}", package.version),
        Some(source) if source.starts_with("http") || source.starts_with("git+") => {
            let source = source.trim_start_matches("git+");
            let (url, reference) = match source.split_once('#') {
                Some((before, _precise)) => before.split_once('?').unwrap_or((before, "")),
                None => source.split_once('?').unwrap_or((source, "")),
            };
            // `cargo tree` prints a reference percent-encoded (`helio%2Fv30.0.1`).
            let reference = reference.replace("%2F", "/").replace("%2f", "/");
            let reference = match reference.split_once('=') {
                Some(("rev", value)) => format!(", rev = \"{value}\""),
                Some(("branch", value)) => format!(", branch = \"{value}\""),
                Some(("tag", value)) => format!(", tag = \"{value}\""),
                _ => String::new(),
            };
            format!("{{ {rename}git = \"{url}\"{reference}, {features} }}")
        }
        Some(path) => {
            let path = Path::new(path);
            let relative = path
                .strip_prefix(root)
                .map(|rest| format!("../../{}", rest.to_string_lossy().replace('\\', "/")))
                .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
            format!("{{ {rename}path = \"{relative}\", {features} }}")
        }
    }
}

fn generate(root: &Path) -> Result<String> {
    let roots = plugin_roots(root)?;
    let libraries: Vec<String> = SHARED_LIBRARIES.iter().map(|p| p.to_string()).collect();

    // The crates inside the shared libraries, per platform. `sdk_roots` depends on all six libraries, so one
    // `cargo tree` per platform gives them (the libraries live in nested workspaces and cargo cannot select
    // packages of several of them in one command, which is what that package is for). Which platform a crate
    // belongs to is decided here, from the libraries' own graph: the engine's graph lists crates a platform does
    // not build (a Windows-only crate reached through an optional dependency), and pinning one of those would
    // compile it where it cannot build.
    let skip: Vec<&str> = libraries.iter().map(String::as_str).chain(["sdk_roots", "pulsar_workspace_hack"]).collect();
    let mut inside: BTreeMap<&str, BTreeSet<Package>> = BTreeMap::new();
    for target in TARGETS {
        let packages = inside.entry(target).or_default();
        for (package, _) in cargo_tree(root, target, &["-p".to_owned(), "sdk_roots".to_owned()])? {
            if !skip.contains(&package.name.as_str()) {
                packages.insert(package);
            }
        }
    }

    // Their features in the build every consumer is part of: the editor, every plugin and the libraries.
    let mut union_args: Vec<String> = vec!["-p".into(), "sdk_roots".into(), "-p".into(), "pulsar_engine".into()];
    for root_crate in &roots {
        union_args.extend(["-p".into(), root_crate.clone()]);
    }
    let features: Vec<String> = roots.iter().map(|r| format!("{r}/dll")).collect();
    union_args.extend(["--features".into(), features.join(",")]);

    // Per platform: what the build of that platform pulls from inside the libraries, with its features.
    let mut per_target: BTreeMap<&str, BTreeMap<Package, BTreeSet<String>>> = BTreeMap::new();
    for target in TARGETS {
        let pinned = per_target.entry(target).or_default();
        for (package, enabled) in cargo_tree(root, target, &union_args)? {
            if inside[target].contains(&package) {
                pinned.entry(package).or_default().extend(enabled);
            }
        }
    }

    // Identical on every platform: the plain table. Otherwise: the platform's own table.
    let mut all: BTreeSet<Package> = BTreeSet::new();
    for pinned in per_target.values() {
        all.extend(pinned.keys().cloned());
    }
    let mut common: BTreeMap<Package, BTreeSet<String>> = BTreeMap::new();
    let mut specific: BTreeMap<&str, BTreeMap<Package, BTreeSet<String>>> = BTreeMap::new();
    for package in &all {
        let sets: Vec<Option<&BTreeSet<String>>> = TARGETS.iter().map(|t| per_target[t].get(package)).collect();
        if sets.iter().all(|s| s.is_some() && *s == sets[0]) {
            common.insert(package.clone(), sets[0].cloned().unwrap_or_default());
        } else {
            for target in TARGETS {
                if let Some(features) = per_target[target].get(package) {
                    specific.entry(target).or_default().insert(package.clone(), features.clone());
                }
            }
        }
    }

    // Two versions of one crate need distinct keys; a key is the same in every table.
    let mut keys: BTreeMap<String, Vec<&Package>> = BTreeMap::new();
    for package in &all {
        keys.entry(package.name.clone()).or_default().push(package);
    }
    let key_of = |package: &Package| -> (String, bool) {
        let renamed = keys[&package.name].len() > 1;
        let key = if renamed {
            format!("{}-{}", package.name, package.version.replace(|c: char| !c.is_ascii_alphanumeric(), "_"))
        } else {
            package.name.clone()
        };
        (key, renamed)
    };
    let table = |pinned: &BTreeMap<Package, BTreeSet<String>>| -> String {
        let mut lines: Vec<String> = pinned
            .iter()
            .map(|(package, features)| {
                let (key, renamed) = key_of(package);
                format!("{key} = {}\n", dependency(package, features, root, renamed))
            })
            .collect();
        lines.sort();
        lines.concat()
    };
    // The same pins twice: as dependencies, and as build dependencies. Under resolver 2 the copy of a crate a
    // build script or proc-macro uses unifies its features apart from the copy the target code uses, so pinning
    // only the second leaves the first to whatever else a build asks for, and its hash (and everything built on
    // it) changes from one cargo invocation to the next.
    let mut body = table(&common);
    for (target, pinned) in &specific {
        body.push_str(&format!("\n[target.{target}.dependencies]\n{}", table(pinned)));
    }
    body.push_str(&format!("\n[build-dependencies]\n{}", table(&common)));
    for (target, pinned) in &specific {
        body.push_str(&format!("\n[target.{target}.build-dependencies]\n{}", table(pinned)));
    }
    let header = virtual_fs::read_to_string(&root.join("tools").join("workspace_hack").join("Cargo.toml.head"))?;
    Ok(format!("{header}{body}"))
}

fn main() -> Result<()> {
    let check = std::env::args().any(|arg| arg == "--check");
    let root = repo_root();
    let manifest = root.join("tools").join("workspace_hack").join("Cargo.toml");
    // The engine depends on the hack, so resolving with the current hack would feed its own pins (every
    // platform's) back into the answer: resolve with it empty, and put the file back if only checking.
    let current = virtual_fs::read_to_string(&manifest).ok();
    let header = virtual_fs::read_to_string(&root.join("tools").join("workspace_hack").join("Cargo.toml.head"))?;
    virtual_fs::write_file(&manifest, header.as_bytes())?;
    let generated = generate(&root);
    if check || generated.is_err() {
        if let Some(current) = &current {
            virtual_fs::write_file(&manifest, current.as_bytes())?;
        }
    }
    let generated = generated?;
    if check {
        if current.is_some_and(|current| current == generated) {
            return Ok(());
        }
        return Err("tools/workspace_hack is out of date: run `cargo run -p hack_gen`".into());
    }
    virtual_fs::write_file(&manifest, generated.as_bytes())?;
    eprintln!("[hack_gen] wrote {}", manifest.display());
    Ok(())
}
