use super::*;

fn plugin(id: &str, requires: &[&str], by_default: bool) -> PluginDescriptor {
    PluginDescriptor {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        enabled_by_default: by_default,
        requires: requires.iter().map(|r| r.to_string()).collect(),
        root: format!("{}_plugin", id.trim_start_matches("p.")),
        editor_only: false,
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
        root = "voxel_plugin"
        "#,
    )
    .unwrap();
    assert_eq!(descriptor.id, "pulsar.voxel");
    assert!(descriptor.enabled_by_default);
    assert!(descriptor.runtime(), "a game links a plugin by default");
    assert!(descriptor.owns("voxel_plugin"));
    assert!(descriptor.owns("voxel-plugin"));
    assert!(
        !descriptor.owns("voxel_component"),
        "only the root registers: the plugin's other crates are not origins"
    );
    assert!(PluginDescriptor::parse("id = \"x\"\nname = \"x\"").is_err());
    assert!(PluginDescriptor::parse("id = \"x\"\nname = \"x\"\nroot = \"a\"\nwhat = 1").is_err());
    let tool = PluginDescriptor::parse(
        "id = \"x\"\nname = \"x\"\nroot = \"x_editor\"\neditor_only = true",
    )
    .unwrap();
    assert!(!tool.runtime(), "a game never links an editor tool");
}

/// A registration type as an owning crate declares one.
struct Probe {
    origin: &'static str,
    name: &'static str,
}

impl Contribution for Probe {
    const CATEGORY: &'static str = "test.probe";
    fn origin(&self) -> &'static str {
        self.origin
    }
    fn id(&self) -> String {
        self.name.to_string()
    }
}

crate::registration!(Probe);

inventory::submit! { Probe { origin: engine_origin!(), name: "engine" } }
inventory::submit! { Probe { origin: "probe_typo_crate", name: "unknown" } }

#[test]
fn every_declared_registration_type_is_a_contribution_source() {
    let probes: Vec<Summary> = contributions()
        .into_iter()
        .filter(|summary| summary.category == "test.probe")
        .collect();
    assert_eq!(
        probes,
        [
            Summary::new("test.probe", "engine", "engine", "engine:pulsar_plugins"),
            Summary::new("test.probe", "unknown", "unknown", "probe_typo_crate"),
        ]
    );
    assert_eq!(registered::<Probe>().count(), 2);
}

#[test]
fn an_origin_is_a_plugin_the_engine_or_unknown() {
    assert_eq!(owner_of(engine_origin!()), Owner::Engine("pulsar_plugins"));
    // No plugin is installed in this test binary: a package name is unknown,
    // not the engine's.
    assert_eq!(owner_of("voxel_plugin"), Owner::Unknown("voxel_plugin"));
    let unknown = unknown_origins();
    assert_eq!(
        unknown
            .get("probe_typo_crate")
            .map(|categories| categories.len()),
        Some(1)
    );
    assert!(!unknown.contains_key("engine:pulsar_plugins"));
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
fn runtime_roots_are_the_enabled_plugins_a_game_links() {
    let mut tool = plugin("p.tool", &[], true);
    tool.editor_only = true;
    let installed = [plugin("p.a", &[], true), plugin("p.b", &[], true), tool];
    let resolution = resolve(&installed, &ProjectPlugins::default().with(&["p.a"], false));
    assert!(resolution.is_enabled("p.tool"));
    assert_eq!(resolution.runtime_roots(&installed), ["b_plugin"]);
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
