use std::{env, fs, path::Path, path::PathBuf};

const HELIO_DEPENDENCIES: [&str; 5] = [
    "helio",
    "helio-asset-compat",
    "helio-default-graphs",
    "helio-planet-voxel-core",
    "helio-pass-planetary-voxel",
];

/// Dependencies a generated game project needs, resolved from *this* engine
/// build's `[workspace.dependencies]` so the game compiles against the exact
/// same crate versions/sources as the editor.
///
/// This matters doubly for Play-In-Editor (issue #243): the game is loaded into
/// the editor process and shares its `wgpu` device across a dylib boundary, so a
/// version skew (e.g. a different `helio`/`wgpu` pulled from a newer git commit)
/// would be undefined behaviour. Pinning to the engine's own resolved sources
/// removes that risk and fixes the ecosystem version-skew that plain
/// `git = "…Pulsar-Native"` (no rev) otherwise causes.
const GAME_DEPENDENCIES: &[&str] = &[
    "pulsar_game",
    "pulsar_scenedb",
    "pulsar_std",
    "pulsar_pie_abi",
    "engine_class_derive",
    "pulsar_reflection",
    "helio",
    "serde",
    "serde_json",
    "tracing",
    "tracing-subscriber",
    "winit",
];

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    // engine_backend lives at crates/core/engine_backend → workspace root is
    // three levels up.
    let workspace_root = manifest_dir.join("../../..");
    let workspace_manifest = workspace_root.join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", workspace_manifest.display());

    let source = fs::read_to_string(&workspace_manifest).unwrap_or_else(|error| {
        panic!(
            "failed to read Pulsar workspace manifest {}: {error}",
            workspace_manifest.display()
        )
    });
    let manifest: toml::Value = toml::from_str(&source)
        .unwrap_or_else(|error| panic!("failed to parse Pulsar workspace manifest: {error}"));
    let dependencies = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table)
        .expect("Pulsar workspace manifest is missing [workspace.dependencies]");

    // ── Helio revision (kept for HELIO_GIT_REVISION consumers) ───────────────
    let revision = dependency_revision(dependencies, HELIO_DEPENDENCIES[0]);
    for dependency in HELIO_DEPENDENCIES.into_iter().skip(1) {
        let candidate = dependency_revision(dependencies, dependency);
        assert_eq!(
            candidate, revision,
            "workspace dependency `{dependency}` uses Helio revision {candidate}, expected {revision}"
        );
    }
    println!("cargo:rustc-env=PULSAR_HELIO_GIT_REVISION={revision}");

    // ── Bake the generated game's [dependencies] + [patch] ───────────────────
    // `core_project_builder.rs` `include_str!`s this into every generated
    // Cargo.toml. Workspace-relative `path` deps are resolved to absolute paths
    // so the out-of-tree game project resolves them, and the full `[patch]`
    // table is carried over (paths absolutized) so the whole Pulsar ecosystem
    // (pbgc, graphy, gpui, …) unifies to single local copies instead of being
    // re-fetched at mismatched git commits.
    let manifest_deps = build_game_manifest_deps(&manifest, dependencies, &workspace_root);
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let deps_path = out_dir.join("game_manifest_deps.toml");
    fs::write(&deps_path, manifest_deps)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", deps_path.display()));
}

fn dependency_revision<'a>(dependencies: &'a toml::Table, dependency: &str) -> &'a str {
    dependencies
        .get(dependency)
        .and_then(|value| value.get("rev"))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| {
            panic!("workspace dependency `{dependency}` must pin Helio with an explicit `rev`")
        })
}

/// Assemble the `[dependencies]` + `[patch."…"]` TOML text for generated games.
fn build_game_manifest_deps(
    manifest: &toml::Value,
    dependencies: &toml::Table,
    workspace_root: &Path,
) -> String {
    let mut out = String::new();
    out.push_str(
        "# Dependencies + patches baked from the engine's own workspace at build\n\
         # time (see engine_backend/build.rs) so the game compiles against the\n\
         # exact crate versions this engine uses — required for Play-In-Editor's\n\
         # shared-wgpu ABI. Absolute paths point at the engine checkout.\n",
    );
    out.push_str("[dependencies]\n");
    for dep in GAME_DEPENDENCIES {
        let value = dependencies
            .get(*dep)
            .unwrap_or_else(|| panic!("workspace dependency `{dep}` is required by generated games but missing from [workspace.dependencies]"));
        // The workspace may enable fewer features than the generated code needs
        // (e.g. it pins `tracing-subscriber` with no features, but the generated
        // `main.rs` calls `.with_env_filter`). Ensure the extras the templates
        // rely on without dropping the workspace's version pin.
        let value = with_features(rewrite_paths(value, workspace_root), required_extra_features(dep));
        out.push_str(&format!("{dep} = {}\n", format_toml_inline(&value)));
    }

    if let Some(patch) = manifest.get("patch").and_then(toml::Value::as_table) {
        for (source_url, table) in patch {
            let Some(entries) = table.as_table() else {
                continue;
            };
            out.push_str(&format!("\n[patch.\"{source_url}\"]\n"));
            for (crate_name, spec) in entries {
                let rewritten = rewrite_paths(spec, workspace_root);
                out.push_str(&format!("{crate_name} = {}\n", format_toml_inline(&rewritten)));
            }
        }
    }

    out
}

/// Extra cargo features a generated game requires beyond what the workspace
/// enables for a given crate. Keyed by the game-manifest dependency name.
fn required_extra_features(dep: &str) -> &'static [&'static str] {
    match dep {
        // The generated `main.rs` uses `fmt().with_env_filter(...)`.
        "tracing-subscriber" => &["fmt", "env-filter"],
        // The generated class code derives serde traits.
        "serde" => &["derive"],
        _ => &[],
    }
}

/// Ensure `value` (a dependency spec) enables `extra` features, converting a bare
/// version string into a table when needed and merging without duplicates. Git/
/// path table specs pass through when `extra` is empty.
fn with_features(value: toml::Value, extra: &[&str]) -> toml::Value {
    if extra.is_empty() {
        return value;
    }
    let mut table = match value {
        toml::Value::String(version) => {
            let mut t = toml::value::Table::new();
            t.insert("version".to_string(), toml::Value::String(version));
            t
        }
        toml::Value::Table(t) => t,
        other => return other,
    };
    let mut features: Vec<String> = table
        .get("features")
        .and_then(toml::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for feature in extra {
        if !features.iter().any(|f| f == feature) {
            features.push((*feature).to_string());
        }
    }
    table.insert(
        "features".to_string(),
        toml::Value::Array(features.into_iter().map(toml::Value::String).collect()),
    );
    toml::Value::Table(table)
}

/// Recursively rewrite any `path = "<relative>"` entry to an absolute path under
/// `workspace_root`, leaving everything else untouched.
fn rewrite_paths(value: &toml::Value, workspace_root: &Path) -> toml::Value {
    match value {
        toml::Value::Table(table) => {
            let mut out = toml::value::Table::new();
            for (key, val) in table {
                if key == "path" {
                    if let Some(rel) = val.as_str() {
                        out.insert(key.clone(), toml::Value::String(absolute_path(workspace_root, rel)));
                        continue;
                    }
                }
                out.insert(key.clone(), rewrite_paths(val, workspace_root));
            }
            toml::Value::Table(out)
        }
        toml::Value::Array(items) => {
            toml::Value::Array(items.iter().map(|v| rewrite_paths(v, workspace_root)).collect())
        }
        other => other.clone(),
    }
}

/// Resolve `rel` against `workspace_root`, canonicalizing when possible, and
/// return a forward-slashed string TOML/Cargo accepts on every platform.
fn absolute_path(workspace_root: &Path, rel: &str) -> String {
    let joined = workspace_root.join(rel);
    let resolved = joined.canonicalize().unwrap_or(joined);
    resolved.to_string_lossy().replace('\\', "/")
}

/// Format a `toml::Value` as inline TOML suitable for the right-hand side of a
/// dependency line (`name = <this>`).
fn format_toml_inline(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => format!("\"{}\"", escape_toml_string(s)),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => format!("\"{d}\""),
        toml::Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(format_toml_inline).collect();
            format!("[{}]", parts.join(", "))
        }
        toml::Value::Table(table) => {
            let parts: Vec<String> = table
                .iter()
                .map(|(k, v)| format!("{k} = {}", format_toml_inline(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

fn escape_toml_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
