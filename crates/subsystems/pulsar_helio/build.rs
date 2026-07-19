use std::{env, fs, path::PathBuf};

const HELIO_DEPENDENCIES: [&str; 4] = [
    "helio",
    "helio-snapshot",
    "helio-asset-compat",
    "helio-default-graphs",
];

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let workspace_manifest = manifest_dir.join("../../../Cargo.toml");
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

    let revision = dependency_revision(dependencies, HELIO_DEPENDENCIES[0]);
    for dependency in HELIO_DEPENDENCIES.into_iter().skip(1) {
        let candidate = dependency_revision(dependencies, dependency);
        assert_eq!(
            candidate, revision,
            "workspace dependency `{dependency}` uses Helio revision {candidate}, expected {revision}"
        );
    }

    println!("cargo:rustc-env=PULSAR_HELIO_GIT_REVISION={revision}");
}

fn dependency_revision<'a>(dependencies: &'a toml::Table, dependency: &str) -> &'a str {
    let entry = dependencies
        .get(dependency)
        .unwrap_or_else(|| panic!("workspace missing dependency `{dependency}`"));
    // Git-based: extract `rev` from the git URL.
    if let Some(rev) = entry.get("rev").and_then(toml::Value::as_str) {
        return rev;
    }
    // Path-based: use a fixed revision (local checkout).
    if entry.get("path").is_some() {
        return "local-dev";
    }
    panic!("workspace dependency `{dependency}` must use `git` with `rev` or `path`")
}
