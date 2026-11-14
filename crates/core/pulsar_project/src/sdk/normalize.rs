//! Normalizing a mirrored package's manifest, the way `cargo package` does,
//! so the package stands alone outside the engine workspace:
//!
//! - inherited values (`x.workspace = true`, in `[package]` and in every
//!   dependency table) are replaced by the engine workspace's, with path
//!   dependencies rebased from the workspace root onto the package;
//! - what only the engine workspace builds is removed: dev-dependencies,
//!   lints (an SDK package is a dependency: its warnings are not the game's
//!   and its `deny`s must not fail a game build), test, bench and example
//!   targets (their directories are not mirrored), and the tables only a
//!   workspace root reads (`[workspace]`, `[patch]`, `[profile]`,
//!   `[replace]`);
//! - `[package]` paths to files that are not mirrored (`readme`,
//!   `license-file`) and `package.workspace` are removed.
//!
//! Normalizing a normalized manifest changes nothing, so an exported SDK
//! (already normalized) mirrors through the same code.

use super::index::SdkWorkspace;
use crate::paths;

/// The first lines of every mirrored manifest.
pub const HEADER: &str = "# Mirrored from the engine SDK by Pulsar and normalized (inherited\n\
# workspace values resolved; dev-dependencies, lints, tests, benches and\n\
# examples removed). Rebuilt on every machine: do not edit.\n";

const ROOT_ONLY: &[&str] = &["workspace", "patch", "profile", "replace", "lints", "badges"];
const DEV_TABLES: &[&str] = &["dev-dependencies", "dev_dependencies"];
const DEP_TABLES: &[&str] = &[
    "dependencies",
    "build-dependencies",
    "build_dependencies",
];
const TARGET_LISTS: &[&str] = &["test", "bench", "example"];
const DROPPED_PACKAGE_KEYS: &[&str] = &["workspace", "readme", "license-file", "license_file"];

/// Normalize `manifest`, the manifest of the package at `package_dir`
/// (relative to the SDK root), against the engine workspace `ws`.
pub fn normalize(manifest: &str, package_dir: &str, ws: &SdkWorkspace) -> Result<String, String> {
    let body = manifest.strip_prefix(HEADER).unwrap_or(manifest);
    let mut doc: toml::Table = toml::from_str(body).map_err(|e| e.to_string())?;

    for key in ROOT_ONLY.iter().chain(DEV_TABLES).chain(TARGET_LISTS) {
        doc.remove(*key);
    }

    let package = doc
        .get_mut("package")
        .and_then(toml::Value::as_table_mut)
        .ok_or("no [package] table")?;
    for key in DROPPED_PACKAGE_KEYS {
        package.remove(*key);
    }
    let inherited: Vec<String> = package
        .iter()
        .filter(|(_, value)| is_inherited(value))
        .map(|(key, _)| key.clone())
        .collect();
    for key in inherited {
        let value = ws
            .package
            .get(&key)
            .ok_or_else(|| format!("`package.{key}` is inherited, but the workspace has none"))?;
        package.insert(key, value.clone());
    }
    for key in ["autotests", "autobenches", "autoexamples"] {
        package.insert(key.to_owned(), toml::Value::Boolean(false));
    }

    for table in DEP_TABLES {
        if let Some(deps) = doc.get_mut(*table).and_then(toml::Value::as_table_mut) {
            resolve_dependencies(deps, package_dir, ws)?;
        }
    }
    if let Some(targets) = doc.get_mut("target").and_then(toml::Value::as_table_mut) {
        for (_, target) in targets.iter_mut() {
            let Some(target) = target.as_table_mut() else {
                continue;
            };
            for key in DEV_TABLES {
                target.remove(*key);
            }
            for table in DEP_TABLES {
                if let Some(deps) = target.get_mut(*table).and_then(toml::Value::as_table_mut) {
                    resolve_dependencies(deps, package_dir, ws)?;
                }
            }
        }
    }

    let body = toml::to_string(&doc).map_err(|e| e.to_string())?;
    Ok(format!("{HEADER}\n{body}"))
}

fn is_inherited(value: &toml::Value) -> bool {
    value
        .as_table()
        .and_then(|t| t.get("workspace"))
        .and_then(toml::Value::as_bool)
        == Some(true)
}

fn resolve_dependencies(
    deps: &mut toml::Table,
    package_dir: &str,
    ws: &SdkWorkspace,
) -> Result<(), String> {
    for (name, spec) in deps.iter_mut() {
        if !is_inherited(spec) {
            continue;
        }
        let member = spec.as_table().expect("an inherited spec is a table").clone();
        let base = ws
            .dependencies
            .get(name)
            .ok_or_else(|| format!("dependency `{name}` is inherited, but the workspace has none"))?;
        let mut resolved = match base {
            toml::Value::String(version) => {
                let mut t = toml::Table::new();
                t.insert("version".into(), toml::Value::String(version.clone()));
                t
            }
            toml::Value::Table(t) => t.clone(),
            _ => return Err(format!("workspace dependency `{name}` is not a version or a table")),
        };
        if let Some(path) = resolved.get("path").and_then(toml::Value::as_str) {
            let target = paths::normalize_rel(path).ok_or_else(|| {
                format!("workspace dependency `{name}` has a path outside the workspace: {path}")
            })?;
            resolved.insert(
                "path".into(),
                toml::Value::String(paths::relative(package_dir, &target)),
            );
        }
        for (key, value) in member {
            match key.as_str() {
                "workspace" => {}
                "features" => {
                    let mut features: Vec<toml::Value> = resolved
                        .get("features")
                        .and_then(toml::Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for feature in value.as_array().into_iter().flatten() {
                        if !features.contains(feature) {
                            features.push(feature.clone());
                        }
                    }
                    resolved.insert(key, toml::Value::Array(features));
                }
                _ => {
                    resolved.insert(key, value);
                }
            }
        }
        *spec = toml::Value::Table(resolved);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> SdkWorkspace {
        toml::from_str::<toml::Table>(
            r#"
            [package]
            version = "0.3.0"
            edition = "2021"
            [dependencies]
            serde = { version = "1", features = ["derive"] }
            log = "0.4"
            pulsar_std = { path = "crates/core/pulsar_std" }
            "#,
        )
        .map(|t| SdkWorkspace {
            package: t["package"].as_table().unwrap().clone(),
            dependencies: t["dependencies"].as_table().unwrap().clone(),
            ..Default::default()
        })
        .unwrap()
    }

    const MANIFEST: &str = r#"
[package]
name = "lights_plugin"
version.workspace = true
edition = { workspace = true }
readme = "../../README.md"

[lints]
workspace = true

[features]
editor = ["dep:lights_editor"]

[dependencies]
serde = { workspace = true, features = ["rc"], optional = true }
log.workspace = true
pulsar_std = { workspace = true }
lights_component = { path = "component" }
lights_editor = { path = "editor", optional = true }

[target.'cfg(windows)'.dependencies]
pulsar_std = { workspace = true }

[target.'cfg(windows)'.dev-dependencies]
tempfile = "3"

[dev-dependencies]
installed_plugins = { workspace = true }

[[test]]
name = "lights"
path = "tests/lights.rs"

[workspace]
"#;

    #[test]
    fn inherited_values_resolve_and_workspace_only_tables_go() {
        let out = normalize(MANIFEST, "plugins/vendor/lights", &workspace()).unwrap();
        assert!(out.starts_with(HEADER));
        let doc: toml::Table = toml::from_str(&out).unwrap();
        let package = doc["package"].as_table().unwrap();
        assert_eq!(package["version"].as_str(), Some("0.3.0"));
        assert_eq!(package["edition"].as_str(), Some("2021"));
        assert!(!package.contains_key("readme"));
        assert_eq!(package["autotests"].as_bool(), Some(false));
        for gone in ["lints", "dev-dependencies", "test", "workspace"] {
            assert!(!doc.contains_key(gone), "{gone} is kept");
        }

        let deps = doc["dependencies"].as_table().unwrap();
        let serde = deps["serde"].as_table().unwrap();
        assert_eq!(serde["version"].as_str(), Some("1"));
        assert_eq!(
            serde["features"].as_array().unwrap(),
            &vec![toml::Value::from("derive"), toml::Value::from("rc")]
        );
        assert_eq!(serde["optional"].as_bool(), Some(true));
        assert_eq!(deps["log"]["version"].as_str(), Some("0.4"));
        assert_eq!(
            deps["pulsar_std"]["path"].as_str(),
            Some("../../../crates/core/pulsar_std")
        );
        assert_eq!(deps["lights_component"]["path"].as_str(), Some("component"));
        let windows = doc["target"]["cfg(windows)"].as_table().unwrap();
        assert!(!windows.contains_key("dev-dependencies"));
        assert_eq!(
            windows["dependencies"]["pulsar_std"]["path"].as_str(),
            Some("../../../crates/core/pulsar_std")
        );
        assert!(!out.contains("workspace = true"));
    }

    #[test]
    fn normalizing_is_idempotent() {
        let once = normalize(MANIFEST, "plugins/vendor/lights", &workspace()).unwrap();
        let twice = normalize(&once, "plugins/vendor/lights", &workspace()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn an_inherited_value_the_workspace_lacks_is_an_error() {
        let manifest = "[package]\nname = \"x\"\nlicense.workspace = true\n";
        let error = normalize(manifest, "x", &workspace()).unwrap_err();
        assert!(error.contains("package.license"), "{error}");
    }
}
