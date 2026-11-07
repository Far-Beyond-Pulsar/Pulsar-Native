//! The installed set is the plugins under `plugins/vendor` that describe
//! themselves (`pulsar-plugin.toml`): this crate's dependencies, its
//! `editor` feature and its re-exports are generated from those
//! descriptors. Fails when they drift; `UPDATE_INSTALLED_PLUGINS=1` rewrites
//! them. Also checks that every linked descriptor is installed, and that a
//! plugin's registrations come only from crates its descriptor lists (the
//! gate maps a registration's origin to its plugin through that list).

use std::path::{Path, PathBuf};

use pulsar_plugins::PluginDescriptor;

// Linked, so its runtime modules install their plugins here.
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
    found
}

fn ident(package: &str) -> String {
    package.replace('-', "_")
}

/// The generated lines of each block, without the markers.
fn blocks(plugins: &[(PathBuf, PluginDescriptor)]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut dependencies = Vec::new();
    let mut features = vec!["editor = [".to_string()];
    let mut reexports = Vec::new();
    for (_, plugin) in plugins {
        for module in &plugin.modules.runtime {
            dependencies.push(format!("{module} = {{ workspace = true }}"));
            reexports.push(format!("pub use {};", ident(module)));
        }
        for module in &plugin.modules.editor {
            dependencies.push(format!(
                "{module} = {{ workspace = true, optional = true }}"
            ));
            features.push(format!("    \"dep:{module}\","));
            reexports.push("#[cfg(feature = \"editor\")]".to_string());
            reexports.push(format!("pub use {};", ident(module)));
        }
    }
    features.push("]".to_string());
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
            for line in lines.by_ref() {
                if line.trim_start() == format!("{comment} {END}") {
                    out.push(line.to_string());
                    break;
                }
            }
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
fn every_descriptor_is_installed_by_its_runtime_module() {
    let linked: Vec<&str> = pulsar_plugins::installed()
        .iter()
        .map(|plugin| plugin.id.as_str())
        .collect();
    for (dir, plugin) in descriptors() {
        assert!(
            linked.contains(&plugin.id.as_str()),
            "{} is not installed: its module `{}` must call \
             `pulsar_plugins::installed!(\"../pulsar-plugin.toml\")`",
            dir.display(),
            plugin.installing_module()
        );
        for required in &plugin.requires {
            assert!(
                linked.contains(&required.as_str()),
                "{} requires unknown `{required}`",
                plugin.id
            );
        }
    }
}

/// The package name of the crate a source file belongs to.
fn package_of(file: &Path) -> Option<String> {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Ok(manifest) = std::fs::read_to_string(d.join("Cargo.toml")) {
            let manifest: toml::Table = toml::from_str(&manifest).ok()?;
            return manifest
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(|name| name.as_str())
                .map(str::to_string);
        }
        dir = d.parent();
    }
    None
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
                .is_some_and(|n| n != "target" && n != "tests")
            {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn a_plugin_registers_only_from_its_modules() {
    for (dir, plugin) in descriptors() {
        let mut files = Vec::new();
        rust_files(&dir, &mut files);
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            if !source.contains("pulsar_plugins::origin!()") {
                continue;
            }
            let package = package_of(&file).expect("a crate");
            assert!(
                plugin.owns(&package),
                "{} submits registrations from `{package}`, which {} does not list as a \
                 module: the gate would never skip them",
                file.display(),
                plugin.id
            );
        }
    }
}
