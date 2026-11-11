//! The installed set is the plugins under `plugins/vendor` that describe
//! themselves (`pulsar-plugin.toml`): this crate's dependencies, its
//! `editor` feature and its re-exports are generated from those
//! descriptors. Fails when they drift; `UPDATE_INSTALLED_PLUGINS=1` rewrites
//! them. Also checks that every linked descriptor is installed by its root
//! crate, and that a plugin registers only from its root (the gate maps a
//! registration's origin to its plugin through the root).

use std::path::{Path, PathBuf};

use pulsar_plugins::PluginDescriptor;

// Linked, so the plugins' root crates install them here.
use installed_plugins as _;

const BEGIN: &str = "BEGIN installed plugins: generated from plugins/vendor/*/pulsar-plugin.toml";
const END: &str = "END installed plugins";
const HOW: &str =
    "by `UPDATE_INSTALLED_PLUGINS=1 cargo test -p installed_plugins --test installed_set`.";

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn vendor_dir() -> PathBuf {
    crate_dir().join("../vendor")
}

/// Every descriptor under plugins/vendor, with its plugin directory, by id.
fn descriptors() -> Vec<(PathBuf, PluginDescriptor)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(vendor_dir()).expect("plugins/vendor") {
        let dir = entry.unwrap().path();
        let file = dir.join(pulsar_plugins::DESCRIPTOR_FILE);
        if let Ok(source) = std::fs::read_to_string(&file) {
            let descriptor = PluginDescriptor::parse(&source)
                .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
            found.push((dir, descriptor));
        }
    }
    found.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    // Two plugins with one id or one root crate would generate a duplicate
    // dependency and re-export; the gate could not tell them apart either.
    for pair in found.windows(2) {
        assert!(
            pair[0].1.id != pair[1].1.id,
            "{} and {} both describe the plugin `{}`",
            pair[0].0.display(),
            pair[1].0.display(),
            pair[0].1.id
        );
    }
    let mut roots: Vec<(&str, &Path)> = found
        .iter()
        .map(|(dir, plugin)| (plugin.root.as_str(), dir.as_path()))
        .collect();
    roots.sort();
    for pair in roots.windows(2) {
        assert!(
            pair[0].0 != pair[1].0,
            "{} and {} both name the root crate `{}`",
            pair[0].1.display(),
            pair[1].1.display(),
            pair[0].0
        );
    }
    found
}

fn ident(package: &str) -> String {
    package.replace('-', "_")
}

/// The generated lines of each block, without the markers: each plugin's
/// root crate (an editor tool's only with `editor`), and the `editor`
/// feature turning on every root's editor registrations.
fn blocks(plugins: &[(PathBuf, PluginDescriptor)]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut dependencies = Vec::new();
    let mut features = vec!["editor = [".to_string()];
    // (crate, editor only): re-exported in crate order, as rustfmt orders
    // `use` items, so the generated file is formatted.
    let mut uses = Vec::new();
    for (_, plugin) in plugins {
        let root = &plugin.root;
        if plugin.editor_only {
            dependencies.push(format!("{root} = {{ workspace = true, optional = true }}"));
            features.push(format!("    \"dep:{root}\","));
        } else {
            dependencies.push(format!("{root} = {{ workspace = true }}"));
            features.push(format!("    \"{root}/editor\","));
        }
        uses.push((ident(root), plugin.editor_only));
    }
    features.push("]".to_string());
    uses.sort();
    let mut reexports = Vec::new();
    for (module, editor) in uses {
        if editor {
            reexports.push("#[cfg(feature = \"editor\")]".to_string());
        }
        reexports.push(format!("pub use {module};"));
    }
    (dependencies, features, reexports)
}

/// `source` with every marked block (comment prefix `comment`) replaced by
/// the next of `blocks`, in order.
fn replaced(source: &str, comment: &str, blocks: &[&[String]]) -> String {
    let mut out = Vec::new();
    let mut lines = source.lines();
    let mut next = blocks.iter();
    while let Some(line) = lines.next() {
        out.push(line.to_string());
        if line.trim_start() == format!("{comment} {BEGIN}") {
            out.push(format!("{comment} {HOW}"));
            for generated in *next.next().expect("a block for every marker") {
                out.push(generated.clone());
            }
            // Without the end marker the rest of the file would be dropped
            // as if it were the old block.
            let closed = lines.by_ref().any(|line| {
                let closing = line.trim_start() == format!("{comment} {END}");
                if closing {
                    out.push(line.to_string());
                }
                closing
            });
            assert!(
                closed,
                "`{comment} {BEGIN}` has no closing `{comment} {END}` marker"
            );
        }
    }
    assert!(next.next().is_none(), "a marker for every block");
    out.join("\n") + "\n"
}

fn check(path: &Path, expected: String, drift: &mut Vec<String>) {
    let actual = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    if actual != expected {
        if std::env::var_os("UPDATE_INSTALLED_PLUGINS").is_some() {
            std::fs::write(path, expected).unwrap();
        } else {
            drift.push(path.display().to_string());
        }
    }
}

#[test]
fn the_installed_set_is_generated_from_the_descriptors() {
    let plugins = descriptors();
    assert!(
        plugins.len() >= 16,
        "the object kinds' plugins describe themselves"
    );
    let (dependencies, features, reexports) = blocks(&plugins);
    let manifest = crate_dir().join("Cargo.toml");
    let lib = crate_dir().join("src/lib.rs");
    let mut drift = Vec::new();
    let source = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("\r\n", "\n");
    check(
        &manifest,
        replaced(&source, "#", &[&dependencies, &features]),
        &mut drift,
    );
    let source = std::fs::read_to_string(&lib).unwrap().replace("\r\n", "\n");
    check(&lib, replaced(&source, "//", &[&reexports]), &mut drift);
    assert!(
        drift.is_empty(),
        "{drift:?} differ from the descriptors; rerun with UPDATE_INSTALLED_PLUGINS=1"
    );
}

#[test]
fn every_descriptor_is_installed_by_its_root_crate() {
    let linked: Vec<&str> = pulsar_plugins::installed()
        .iter()
        .map(|plugin| plugin.id.as_str())
        .collect();
    for (dir, plugin) in descriptors() {
        assert_eq!(
            package_of(&dir.join("src/lib.rs")).as_deref(),
            Some(plugin.root.as_str()),
            "{}: `root` names the package at the plugin's directory",
            dir.display()
        );
        assert!(
            linked.contains(&plugin.id.as_str()),
            "{} is not installed: its root crate `{}` must call \
             `pulsar_plugins::installed!(\"pulsar-plugin.toml\")`",
            dir.display(),
            plugin.root
        );
    }
}

/// The package name of the crate a source file belongs to.
fn package_of(file: &Path) -> Option<String> {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Some(package) = package_at(d) {
            return Some(package);
        }
        if d.join("Cargo.toml").exists() {
            return None;
        }
        dir = d.parent();
    }
    None
}

/// The name of the package whose manifest is in `dir`, if any.
fn package_at(dir: &Path) -> Option<String> {
    let manifest: toml::Table =
        toml::from_str(&std::fs::read_to_string(dir.join("Cargo.toml")).ok()?).ok()?;
    manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .map(str::to_string)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n != "target" && n != "tests" && n != ".git")
            {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every package under `plugins/vendor`, with the plugin directory it is in.
fn plugin_packages() -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, plugin: &Path, out: &mut Vec<(String, PathBuf)>) {
        if let Some(package) = package_at(dir) {
            out.push((package, plugin.to_path_buf()));
        }
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|n| n != "target" && n != ".git")
            {
                walk(&path, plugin, out);
            }
        }
    }
    let mut out = Vec::new();
    for (dir, _) in descriptors() {
        walk(&dir, &dir, &mut out);
    }
    out
}

/// What submits a plugin contribution: a registration naming its origin, a
/// world component class submission or the descriptor. A plugin's root crate
/// is the one place it contributes. (Type attachments, such as SceneDB's GPU
/// mirror hooks and reflection's method tables, name no origin: they are
/// metadata of one type and stay with it, inert while the plugin is
/// disabled because its classes are not registered.)
const SUBMITTING: [&str; 5] = [
    "pulsar_plugins::origin!()",
    "p.origin()",
    "p.components()",
    "submit_world_component!",
    "pulsar_plugins::installed!",
];

/// Whether `source` registers something: a line of code (not a comment)
/// with a submitting marker. A registration a unit-test build makes for
/// itself (`#[cfg(test)]` on the line before: the test binary of a crate
/// that is not the root links no root) is not one a build of the plugin makes.
fn submits(source: &str) -> bool {
    let mut previous = "";
    for line in source.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        let code = !line.starts_with("//");
        if code
            && previous != "#[cfg(test)]"
            && SUBMITTING.iter().any(|marker| line.contains(marker))
        {
            return true;
        }
        previous = line;
    }
    false
}

#[test]
fn a_plugin_registers_only_from_its_root_crate() {
    for (dir, plugin) in descriptors() {
        let mut files = Vec::new();
        rust_files(&dir, &mut files);
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            let package = package_of(&file).expect("a crate");
            if submits(&source) {
                assert!(
                    plugin.owns(&package),
                    "{} registers from `{package}`, not from {}'s root crate `{}`: move the \
                     registration to the root (its origin would be a crate the gate does not \
                     know, so it could not skip it while the plugin is disabled)",
                    file.display(),
                    plugin.id,
                    plugin.root
                );
            }
            assert!(
                !source.contains("#[register_world_component"),
                "{}: a plugin's component is `#[world_component]`, registered by its root's \
                 build (`p.components().class::<T>(spec)`)",
                file.display()
            );
        }
    }
}

/// Every linked registration names its origin; one from a crate inside a
/// plugin must come from that plugin's root, so it maps to its plugin and
/// is skipped while the plugin is disabled. An origin outside every plugin
/// is the engine's, kept with that provenance.
#[test]
fn every_contribution_from_a_plugin_comes_from_its_root() {
    let packages = plugin_packages();
    let contributions = pulsar_plugins::contributions();
    assert!(
        contributions
            .iter()
            .any(|summary| summary.category == "render.features"),
        "the installed plugins' registrations are linked"
    );
    for summary in &contributions {
        let Some((_, dir)) = packages
            .iter()
            .find(|(package, _)| package.replace('-', "_") == summary.origin.replace('-', "_"))
        else {
            continue;
        };
        assert!(
            pulsar_plugins::plugin_of(summary.origin).is_some(),
            "`{}` `{}` comes from `{}`, a crate of the plugin at {} that is not its root",
            summary.category,
            summary.id,
            summary.origin,
            dir.display()
        );
    }
}

/// Every class a plugin defines with `#[world_component]` is registered by
/// its root's build (`p.components().class::<T>(spec)`): a class the root
/// forgot would load as unresolved while the plugin is enabled.
#[test]
fn every_world_component_a_plugin_defines_is_registered_by_its_root() {
    let classes: Vec<(String, &'static str)> = pulsar_plugins::contributions()
        .into_iter()
        .filter(|summary| summary.category == "component.class")
        .map(|summary| (summary.id, summary.origin))
        .collect();
    let mut defined = 0;
    for (dir, plugin) in descriptors() {
        let mut files = Vec::new();
        rust_files(&dir, &mut files);
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            let mut lines = source.lines().map(str::trim);
            while let Some(line) = lines.next() {
                if !line.starts_with("#[world_component") {
                    continue;
                }
                // The attribute (possibly over several lines), then `impl Type {}`.
                let Some(class) = lines.by_ref().find_map(|line| {
                    line.strip_prefix("impl ")
                        .map(|rest| rest.split([' ', '{']).next().unwrap_or("").to_string())
                }) else {
                    continue;
                };
                defined += 1;
                assert!(
                    classes
                        .iter()
                        .any(|(id, origin)| *id == class && plugin.owns(origin)),
                    "{}: `{class}` is a `#[world_component]` that {}'s root `{}` does not \
                     register (`p.components().class::<T>(spec)` in its build)",
                    file.display(),
                    plugin.id,
                    plugin.root
                );
            }
        }
    }
    assert!(defined >= 20, "the plugins' classes are found ({defined})");
}

/// A plugin registers through its root's build, never at link time: every
/// registration linked with `inventory::submit!` is the engine's (a plugin's
/// would bypass the build, so a plugin library could not hand it over).
#[test]
fn every_linked_registration_is_the_engines() {
    let plugins: Vec<_> = pulsar_plugins::linked_contributions()
        .into_iter()
        .filter(|summary| {
            !matches!(
                pulsar_plugins::owner_of(summary.origin),
                pulsar_plugins::Owner::Engine(_)
            )
        })
        .map(|summary| {
            format!(
                "{} `{}` from `{}`",
                summary.category, summary.id, summary.origin
            )
        })
        .collect();
    assert!(
        plugins.is_empty(),
        "registered at link time instead of through a root's build:\n{}",
        plugins.join("\n")
    );
    let (built, problems) = pulsar_plugins::builds();
    assert!(problems.is_empty(), "{problems:?}");
    for plugin in pulsar_plugins::installed() {
        assert!(
            built.iter().any(|root| plugin.owns(root)),
            "{}'s root `{}` has no build (`installed!(\"pulsar-plugin.toml\", build)`)",
            plugin.id,
            plugin.root
        );
    }
}
