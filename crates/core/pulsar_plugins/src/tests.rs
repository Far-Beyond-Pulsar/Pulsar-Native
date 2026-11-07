use super::*;

fn plugin(id: &str, requires: &[&str], by_default: bool) -> PluginDescriptor {
    PluginDescriptor {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        enabled_by_default: by_default,
        requires: requires.iter().map(|r| r.to_string()).collect(),
        modules: Modules {
            runtime: vec![format!("{}_component", id.trim_start_matches("p."))],
            editor: vec![format!("{}_plugin", id.trim_start_matches("p."))],
        },
    }
}

fn ids(resolution: &Resolution) -> Vec<&str> {
    resolution.enabled.iter().map(String::as_str).collect()
}

#[test]
fn a_descriptor_parses() {
    let descriptor = PluginDescriptor::parse(
        r#"
        id = "pulsar.voxel"
        name = "Voxel worlds"
        requires = ["pulsar.sky"]
        [modules]
        runtime = ["voxel_component"]
        editor = ["voxel_plugin"]
        "#,
    )
    .unwrap();
    assert_eq!(descriptor.id, "pulsar.voxel");
    assert!(descriptor.enabled_by_default);
    assert!(descriptor.owns("voxel_component"));
    assert!(descriptor.owns("voxel-plugin"));
    assert!(!descriptor.owns("voxel_world"));
    assert!(PluginDescriptor::parse("id = \"x\"\nname = \"x\"\n[modules]\nruntime = []").is_err());
    assert!(PluginDescriptor::parse(
        "id = \"x\"\nname = \"x\"\nwhat = 1\n[modules]\nruntime = [\"a\"]"
    )
    .is_err());
}

#[test]
fn the_project_list_is_its_plugins_table() {
    let list = ProjectPlugins::parse(
        r#"
        # a comment
        [plugins]
        "pulsar.voxel" = false
        "pulsar.lights" = true
        "#,
    )
    .unwrap();
    assert_eq!(list.listed.get("pulsar.voxel"), Some(&false));
    assert_eq!(list.listed.get("pulsar.lights"), Some(&true));
    assert_eq!(
        ProjectPlugins::parse("").unwrap(),
        ProjectPlugins::default()
    );
    assert!(ProjectPlugins::parse("[plugins]\n\"a\" = 1").is_err());
    assert!(ProjectPlugins::parse("plugins = 3").is_err());
}

#[test]
fn defaults_apply_where_the_list_is_silent() {
    let installed = [plugin("p.a", &[], true), plugin("p.b", &[], false)];
    let resolution = resolve(&installed, &ProjectPlugins::default());
    assert_eq!(ids(&resolution), ["p.a"]);
    let resolution = resolve(
        &installed,
        &ProjectPlugins::default()
            .with(&["p.a"], false)
            .with(&["p.b"], true),
    );
    assert_eq!(ids(&resolution), ["p.b"]);
    assert!(resolution.problems.is_empty());
}

#[test]
fn requirements_come_along_unless_the_list_turns_them_off() {
    let installed = [
        plugin("p.top", &["p.base"], false),
        plugin("p.base", &[], false),
        plugin("p.other", &["p.top"], true),
    ];
    // p.other (default on) pulls p.top, which pulls p.base.
    let resolution = resolve(&installed, &ProjectPlugins::default());
    assert_eq!(ids(&resolution), ["p.base", "p.other", "p.top"]);
    assert!(resolution.problems.is_empty());

    // Turning p.base off takes p.top and p.other with it, and says so.
    let resolution = resolve(
        &installed,
        &ProjectPlugins::default().with(&["p.base"], false),
    );
    assert!(ids(&resolution).is_empty());
    assert_eq!(resolution.problems.len(), 2, "{:?}", resolution.problems);
    assert!(resolution.problems[0].contains("requires `p.base`, which the project disables"));
}

#[test]
fn a_missing_requirement_or_plugin_is_a_problem() {
    let installed = [plugin("p.a", &["p.gone"], true)];
    let resolution = resolve(
        &installed,
        &ProjectPlugins::default().with(&["p.nowhere"], true),
    );
    assert!(ids(&resolution).is_empty());
    assert_eq!(resolution.problems.len(), 2, "{:?}", resolution.problems);
    assert!(resolution
        .problems
        .iter()
        .any(|p| p.contains("`p.nowhere` is enabled by the project but not installed")));
    assert!(resolution
        .problems
        .iter()
        .any(|p| p.contains("which is not installed")));
}

#[test]
fn runtime_modules_are_the_enabled_plugins() {
    let installed = [plugin("p.a", &[], true), plugin("p.b", &[], true)];
    let resolution = resolve(&installed, &ProjectPlugins::default().with(&["p.a"], false));
    assert_eq!(resolution.runtime_modules(&installed), ["b_component"]);
}

#[test]
fn packages_compare_with_either_separator() {
    assert!(same_package("helio-pass-gi", "helio_pass_gi"));
    assert!(!same_package("helio-pass-gi", "helio-pass-gix"));
}

#[test]
fn nothing_activated_lets_every_origin_through() {
    // This test binary links no plugin: every origin is the engine's.
    deactivate();
    assert!(origin_enabled("voxel_component"));
    activate(&Resolution::default());
    assert!(
        origin_enabled("voxel_component"),
        "not an installed plugin's module"
    );
    assert!(!is_enabled("pulsar.voxel"));
    deactivate();
}
