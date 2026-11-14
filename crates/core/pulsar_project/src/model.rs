//! The project model: everything the generated files are made from, read
//! from the project's inputs. Rendering ([`crate::render`]) touches no file.
//!
//! Inputs: `Pulsar.toml` (identity, plugins), the scripts crates under
//! `scripts/` (their manifests), the classes whose compile target is Rust
//! (their compiled modules and prefabs), and the SDK index (which plugins
//! exist, which crate names are taken).

use std::collections::BTreeSet;
use std::path::Path;

use engine_fs::virtual_fs;

use crate::descriptor::ProjectDescriptor;
use crate::error::SyncError;
use crate::paths;
use crate::sdk::SdkIndex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectModel {
    /// The game crate's package name (a Rust identifier).
    pub package: String,
    /// The executable's name: the package's.
    pub bin: String,
    /// The Play-In-Editor library's name.
    pub lib: String,
    pub version: String,
    /// Root crates of the enabled runtime plugins, sorted.
    pub plugin_roots: Vec<String>,
    /// Sorted by directory.
    pub scripts: Vec<ScriptCrate>,
    /// Classes compiled to Rust, sorted by module name.
    pub classes: Vec<RustClass>,
    /// What is wrong but does not stop the game building (an unresolved
    /// plugin requirement, a class set to Rust that has no compiled module).
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptCrate {
    /// Its package name.
    pub name: String,
    /// Its directory under `scripts/`.
    pub dir: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustClass {
    /// The module name in the game crate.
    pub module: String,
    /// The generated actor (`pulsar_script_codegen::actor`).
    pub source: String,
}

/// A class's `graph_save.json` key naming its compile target.
pub const COMPILE_TARGET_KEY: &str = "compile_target";
/// The compile target that generates the class into the game crate.
pub const COMPILE_TARGET_RUST: &str = "rust";

impl ProjectModel {
    pub fn resolve(
        project_root: &Path,
        descriptor: &ProjectDescriptor,
        index: &SdkIndex,
    ) -> Result<Self, SyncError> {
        let mut problems = Vec::new();

        let installed = index.plugin_descriptors().map_err(SyncError::Sdk)?;
        let resolution = pulsar_plugins::resolve(&installed, &descriptor.plugins);
        problems.extend(resolution.problems.iter().map(|p| p.to_string()));
        let mut plugin_roots: Vec<String> = resolution
            .runtime_roots(&installed)
            .into_iter()
            .map(str::to_owned)
            .collect();
        plugin_roots.sort();

        let scripts = scripts_crates(project_root, index, &mut problems)?;

        let folder = project_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = descriptor.name.clone().unwrap_or(folder);
        let taken: BTreeSet<&str> = index
            .crates
            .iter()
            .map(String::as_str)
            .chain(scripts.iter().map(|s| s.name.as_str()))
            .collect();
        let package = package_ident(&name, &taken);
        let lib = format!("{package}_pie");

        let version = descriptor.version.clone().unwrap_or_else(|| "0.1.0".to_owned());
        if !is_cargo_version(&version) {
            return Err(SyncError::Descriptor(format!(
                "`project.version` \"{version}\" is not a version like 1.2.3"
            )));
        }

        let classes = rust_classes(project_root, &mut problems);

        Ok(Self {
            bin: package.clone(),
            package,
            lib,
            version,
            plugin_roots,
            scripts,
            classes,
            problems,
        })
    }
}

/// Every directory under `scripts/` with a `Cargo.toml`. A manifest that
/// does not parse, or whose package name another crate has, is an error: the
/// game cannot build with it.
fn scripts_crates(
    project_root: &Path,
    index: &SdkIndex,
    problems: &mut Vec<String>,
) -> Result<Vec<ScriptCrate>, SyncError> {
    let dir = paths::join(project_root, paths::SCRIPTS_DIR);
    let Ok(entries) = virtual_fs::list_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut crates: Vec<ScriptCrate> = Vec::new();
    for entry in entries.into_iter().filter(|e| e.is_dir) {
        let manifest_path = dir.join(&entry.name).join("Cargo.toml");
        if !virtual_fs::is_file(&manifest_path) {
            continue;
        }
        let fail = |message: String| SyncError::ScriptCrate {
            dir: entry.name.clone(),
            message,
        };
        let text = virtual_fs::read_to_string(&manifest_path).map_err(|e| fail(e.to_string()))?;
        let manifest: toml::Table = toml::from_str(&text).map_err(|e| fail(e.to_string()))?;
        let name = manifest
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| fail("its Cargo.toml has no `package.name`".into()))?
            .to_owned();
        if index.crates.contains(&name) {
            return Err(fail(format!(
                "its package name `{name}` is an engine crate's; rename the package"
            )));
        }
        if let Some(other) = crates.iter().find(|c| c.name == name) {
            return Err(fail(format!(
                "its package name `{name}` is also scripts/{}'s",
                other.dir
            )));
        }
        for path in absolute_dependency_paths(&manifest) {
            problems.push(format!(
                "scripts/{}: dependency path `{path}` is absolute, so the project builds only on \
                 this machine; depend on engine crates with `{{ workspace = true }}`",
                entry.name
            ));
        }
        crates.push(ScriptCrate {
            name,
            dir: entry.name,
        });
    }
    crates.sort_by(|a, b| a.dir.cmp(&b.dir));
    Ok(crates)
}

/// The `path` of every dependency in `manifest` that is absolute.
fn absolute_dependency_paths(manifest: &toml::Table) -> Vec<String> {
    let mut tables: Vec<&toml::Table> = ["dependencies", "build-dependencies", "dev-dependencies"]
        .iter()
        .filter_map(|k| manifest.get(*k).and_then(toml::Value::as_table))
        .collect();
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values().filter_map(toml::Value::as_table) {
            tables.extend(
                ["dependencies", "build-dependencies", "dev-dependencies"]
                    .iter()
                    .filter_map(|k| target.get(*k).and_then(toml::Value::as_table)),
            );
        }
    }
    tables
        .into_iter()
        .flat_map(|t| t.values())
        .filter_map(|spec| spec.get("path").and_then(toml::Value::as_str))
        .filter(|path| is_absolute_anywhere(path))
        .map(str::to_owned)
        .collect()
}

/// Absolute on any platform (a project made on Windows is read on Linux).
fn is_absolute_anywhere(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// The classes whose saved compile target is Rust, generated. A class that
/// cannot be generated is a problem, not an error: it still runs as a
/// script.
fn rust_classes(project_root: &Path, problems: &mut Vec<String>) -> Vec<RustClass> {
    #[derive(serde::Deserialize)]
    struct Graph {
        #[serde(default)]
        compile_target: Option<String>,
    }
    let mut classes: Vec<RustClass> = Vec::new();
    for dir in pulsar_class::registry::find_class_dirs(project_root) {
        let graph = dir.join("graph_save.json");
        let Ok(text) = virtual_fs::read_to_string(&graph) else {
            continue;
        };
        let target = match serde_json::from_str::<Graph>(&text) {
            Ok(graph) => graph.compile_target,
            Err(error) => {
                problems.push(format!("{}: {error}", graph.display()));
                continue;
            }
        };
        if target.as_deref() != Some(COMPILE_TARGET_RUST) {
            continue;
        }
        let name = pulsar_class::registry::class_name_of_dir(&dir);
        match generate_class(&dir, &name) {
            Ok(source) => {
                let module = pulsar_script_codegen::actor::snake_case(&name);
                if classes.iter().any(|c| c.module == module) {
                    problems.push(format!(
                        "class `{name}`: another class compiled to Rust has the module name `{module}`; \
                         it runs as a script"
                    ));
                } else {
                    classes.push(RustClass { module, source });
                }
            }
            Err(error) => problems.push(format!(
                "class `{name}` is set to compile to Rust but {error}; it runs as a script"
            )),
        }
    }
    classes.sort_by(|a, b| a.module.cmp(&b.module));
    classes
}

fn generate_class(dir: &Path, name: &str) -> Result<String, String> {
    let module_path = dir.join("events").join(".build").join("module.json");
    let json = virtual_fs::read_to_string(&module_path)
        .map_err(|_| "it has not been compiled (no events/.build/module.json)".to_owned())?;
    let module = pulsar_script_vm::Module::from_json(&json)
        .map_err(|e| format!("its compiled module does not read: {e}"))?;
    let prefab = pulsar_class::prefab::PrefabAsset::load_from_dir(dir)
        .map_err(|e| format!("its prefab does not read: {e}"))?;
    let components: Vec<_> = prefab
        .components
        .iter()
        .map(|c| pulsar_script_codegen::actor::ComponentSpec {
            class_name: c.class_name.clone(),
            property_defaults_json: c.data.to_string(),
            enabled: c.enabled,
        })
        .collect();
    pulsar_script_codegen::actor::generate_actor(name, &module, &components)
        .map_err(|e| format!("it cannot be exported: {e}"))
}

/// A Rust identifier for the game's package from `name`: lowercase ASCII,
/// words joined by `_`, never starting with a digit, never a keyword or a
/// name in `taken` (the engine's crates, the scripts crates).
pub fn package_ident(name: &str, taken: &BTreeSet<&str>) -> String {
    let mut ident = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            ident.push(c.to_ascii_lowercase());
        } else if !ident.ends_with('_') {
            ident.push('_');
        }
    }
    let mut ident = ident.trim_matches('_').to_owned();
    if ident.is_empty() {
        ident = "game".to_owned();
    }
    if ident.starts_with(|c: char| c.is_ascii_digit()) {
        ident.insert_str(0, "game_");
    }
    while RESERVED.contains(&ident.as_str()) || taken.contains(ident.as_str()) {
        ident.push_str("_game");
    }
    ident
}

/// Keywords, and crate names a package must not take.
const RESERVED: &[&str] = &[
    "abstract", "alloc", "as", "async", "await", "become", "box", "break", "const", "continue",
    "core", "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen",
    "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv",
    "proc_macro", "pub", "ref", "return", "self", "static", "std", "struct", "super", "test",
    "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual", "where",
    "while", "yield",
];

/// `MAJOR.MINOR.PATCH`, optionally `-pre` and `+build`.
fn is_cargo_version(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_are_identifiers_that_take_no_other_crate_name() {
        let taken: BTreeSet<&str> = ["helio", "pulsar_game"].into_iter().collect();
        assert_eq!(package_ident("Star Fall: Redux!", &taken), "star_fall_redux");
        assert_eq!(package_ident("voxel-reference-project", &taken), "voxel_reference_project");
        assert_eq!(package_ident("2049", &taken), "game_2049");
        assert_eq!(package_ident("Helio", &taken), "helio_game");
        assert_eq!(package_ident("pulsar game", &taken), "pulsar_game_game");
        assert_eq!(package_ident("test", &taken), "test_game");
        assert_eq!(package_ident("???", &taken), "game");
    }

    #[test]
    fn versions_are_checked() {
        for ok in ["0.1.0", "1.20.3-beta.1", "1.0.0+build5"] {
            assert!(is_cargo_version(ok), "{ok}");
        }
        for bad in ["1.0", "v1.0.0", "1.x.0", ""] {
            assert!(!is_cargo_version(bad), "{bad}");
        }
    }

    #[test]
    fn absolute_paths_are_found_on_any_platform() {
        let manifest: toml::Table = toml::from_str(
            "[dependencies]\na = { path = \"C:/Users/x/Pulsar-Native/crates/core/pulsar_game\" }\n\
             b = { path = \"../b\" }\nc = { workspace = true }\n\
             [target.'cfg(unix)'.dependencies]\nd = { path = \"/home/x/d\" }\n",
        )
        .unwrap();
        let found = absolute_dependency_paths(&manifest);
        assert_eq!(found.len(), 2, "{found:?}");
    }
}
