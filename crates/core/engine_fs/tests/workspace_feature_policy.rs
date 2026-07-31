use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Eq, PartialEq)]
struct DependencyPolicy {
    uses_workspace: bool,
    default_features: bool,
    features: BTreeSet<String>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("engine_fs must remain under crates/core")
        .to_path_buf()
}

fn dependency_policy(value: &toml::Value) -> DependencyPolicy {
    let Some(table) = value.as_table() else {
        return DependencyPolicy {
            uses_workspace: false,
            default_features: true,
            features: BTreeSet::new(),
        };
    };

    let features = table
        .get("features")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .map(|feature| {
            feature
                .as_str()
                .expect("dependency features must be strings")
                .to_owned()
        })
        .collect();

    DependencyPolicy {
        uses_workspace: table
            .get("workspace")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
        default_features: table
            .get("default-features")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true),
        features,
    }
}

fn engine_fs_dependency(manifest: &toml::Value) -> Option<DependencyPolicy> {
    ["dependencies", "dev-dependencies", "build-dependencies"]
        .into_iter()
        .find_map(|section| {
            manifest
                .get(section)
                .and_then(toml::Value::as_table)
                .and_then(|dependencies| dependencies.get("engine_fs"))
                .map(dependency_policy)
        })
}

fn member_manifests(root: &Path) -> Vec<PathBuf> {
    let mut manifests = Vec::new();
    for group in ["core", "editor", "subsystems", "agent-providers"] {
        let group_path = root.join("crates").join(group);
        for entry in fs::read_dir(&group_path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", group_path.display()))
        {
            let path = entry
                .expect("workspace member entry must be readable")
                .path();
            let manifest = path.join("Cargo.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }

    let asset_viewer = root.join("plugins/vendor/asset_viewer/Cargo.toml");
    if asset_viewer.is_file() {
        manifests.push(asset_viewer);
    }
    manifests
}

fn features(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn workspace_engine_fs_callers_declare_their_minimum_surface() {
    let root = workspace_root();
    let root_manifest_source =
        fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest must be readable");
    let root_manifest: toml::Value =
        toml::from_str(&root_manifest_source).expect("workspace manifest must be valid TOML");
    let workspace_dependency = root_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get("engine_fs"))
        .expect("workspace must define engine_fs");
    assert!(
        !dependency_policy(workspace_dependency).default_features,
        "the workspace engine_fs baseline must stay local-only"
    );

    let expected = BTreeMap::from([
        ("agent_chat_tools".to_owned(), features(&["editor"])),
        ("engine_backend".to_owned(), features(&[])),
        ("engine_state".to_owned(), features(&["editor"])),
        ("pulsar_rendering".to_owned(), features(&[])),
        ("pulsar_scene".to_owned(), features(&[])),
        ("pulsar_std".to_owned(), features(&[])),
        ("pulsar_terrain".to_owned(), features(&[])),
        ("ui_common".to_owned(), features(&["editor"])),
        ("ui_entry".to_owned(), features(&["remote"])),
        ("ui_file_manager".to_owned(), features(&["editor"])),
        ("ui_git_manager".to_owned(), features(&[])),
        ("ui_level_editor".to_owned(), features(&[])),
        ("ui_loading_screen".to_owned(), features(&[])),
        ("ui_multiplayer".to_owned(), features(&[])),
        ("ui_type_debugger".to_owned(), features(&["editor"])),
    ]);

    let mut actual = BTreeMap::new();
    for manifest_path in member_manifests(&root) {
        let manifest_source = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
        let manifest: toml::Value = toml::from_str(&manifest_source).unwrap_or_else(|error| {
            panic!(
                "failed to parse {} as TOML: {error}",
                manifest_path.display()
            )
        });
        let Some(policy) = engine_fs_dependency(&manifest) else {
            continue;
        };
        let package = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .expect("workspace member must have a package name");

        if package == "pulsar_std" {
            assert!(
                !policy.uses_workspace && !policy.default_features,
                "pulsar_std must preserve its local-only direct path dependency"
            );
        } else {
            assert!(
                policy.uses_workspace,
                "{package} must inherit the workspace engine_fs baseline"
            );
        }
        actual.insert(package.to_owned(), policy.features);
    }

    assert_eq!(
        actual, expected,
        "classify every engine_fs caller explicitly; new callers must not inherit the full graph"
    );
}
