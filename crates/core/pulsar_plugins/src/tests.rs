use super::*;

/// A plugin providing `<id>.rows` (a buffer contract) and requiring the
/// `.rows` contract of each of `requires`.
fn plugin(id: &str, requires: &[&str], by_default: bool) -> PluginDescriptor {
    PluginDescriptor {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        enabled_by_default: by_default,
        provides: vec![ProvidedContract {
            id: format!("{id}.rows"),
            kind: ContractKind::Buffer,
            members: vec![format!("{id}_rows")],
            version: 1,
        }],
        requires: requires
            .iter()
            .map(|r| RequiredContract {
                id: format!("{r}.rows"),
                optional: false,
                version: None,
            })
            .collect(),
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
        root = "voxel_plugin"
        provides = [
            { id = "pulsar.voxel.rows", kind = "buffer", members = ["voxel_volumes"] },
            { id = "pulsar.voxel.Diggable", kind = "trait" },
        ]
        requires = [
            { id = "pulsar.sky.atmosphere" },
            { id = "pulsar.gi.probes", optional = true, version = 2 },
        ]
        "#,
    )
    .unwrap();
    assert_eq!(descriptor.id, "pulsar.voxel");
    assert!(descriptor.enabled_by_default);
    assert_eq!(
        descriptor.provides[0].version, 1,
        "contracts start at version 1"
    );
    assert_eq!(
        descriptor
            .provider_of(ContractKind::Buffer, "voxel_volumes")
            .map(|c| c.id.as_str()),
        Some("pulsar.voxel.rows")
    );
    assert!(
        descriptor.provides[1].covers("pulsar.voxel.Diggable"),
        "a trait contract with no members is its own trait id"
    );
    assert_eq!(
        descriptor.requires[1],
        RequiredContract {
            id: "pulsar.gi.probes".into(),
            optional: true,
            version: Some(2)
        }
    );
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

    // Contracts are namespaced by their provider and have members; a plugin
    // does not require what it provides; requirements are contracts, never
    // plugin ids.
    let parse = |extra: &str| {
        PluginDescriptor::parse(&format!(
            "id = \"p.a\"\nname = \"a\"\nroot = \"a\"\n{extra}"
        ))
    };
    let rows = r#"provides = [{ id = "p.a.rows", kind = "buffer", members = ["k"] }]"#;
    assert!(parse(rows).is_ok());
    assert!(
        parse(r#"provides = [{ id = "p.b.rows", kind = "buffer", members = ["k"] }]"#)
            .unwrap_err()
            .contains("outside its namespace")
    );
    assert!(
        parse(r#"provides = [{ id = "p.a.rows", kind = "buffer" }]"#)
            .unwrap_err()
            .contains("lists no members")
    );
    assert!(parse(r#"provides = [{ id = "p.a.rows", kind = "pipes", members = ["k"] }]"#).is_err());
    assert!(
        parse(&format!("{rows}\nrequires = [{{ id = \"p.a.rows\" }}]"))
            .unwrap_err()
            .contains("requires its own contract")
    );
    assert!(parse(r#"requires = ["p.b"]"#).is_err());
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
    // Another test imports this binary's own library export, adding its
    // build's registration once more: each registration once here.
    let mut probes: Vec<Summary> = contributions()
        .into_iter()
        .filter(|summary| summary.category == "test.probe")
        .collect();
    probes.dedup();
    assert_eq!(
        probes,
        [
            Summary::new("test.probe", "engine", "engine", "engine:pulsar_plugins"),
            Summary::new("test.probe", "built", "built", "probe_library_plugin"),
            Summary::new("test.probe", "unknown", "unknown", "probe_typo_crate"),
        ]
    );
    // Another test imports this binary's own library export, adding its
    // build's registration once more: count names, not registrations.
    let names: std::collections::BTreeSet<&str> =
        registered::<Probe>().map(|probe| probe.name).collect();
    assert_eq!(names, ["built", "engine", "unknown"].into_iter().collect());
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
    assert_eq!(
        resolution.problems[0],
        Problem::ProviderDisabled {
            consumer: "p.top".into(),
            contract: "p.base.rows".into(),
            provider: "p.base".into(),
            by_project: true,
        }
    );
    assert_eq!(
        resolution.problems[0].to_string(),
        "plugin p.top needs contract p.base.rows, provided by p.base, which the project \
         disables; p.top is disabled"
    );
    assert_eq!(
        resolution.problems[1],
        Problem::ProviderDisabled {
            consumer: "p.other".into(),
            contract: "p.top.rows".into(),
            provider: "p.top".into(),
            by_project: false,
        },
        "a provider dropped for its own requirement is named as disabled"
    );
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
    assert!(resolution.problems.contains(&Problem::NotInstalled {
        plugin: "p.nowhere".into()
    }));
    assert!(resolution.problems.contains(&Problem::Unprovided {
        consumer: "p.a".into(),
        contract: "p.gone.rows".into()
    }));
}

#[test]
fn an_incompatible_provider_is_named_with_both_versions() {
    let mut consumer = plugin("p.a", &["p.b"], true);
    consumer.requires[0].version = Some(2);
    let installed = [consumer, plugin("p.b", &[], true)];
    let resolution = resolve(&installed, &ProjectPlugins::default());
    assert_eq!(ids(&resolution), ["p.b"]);
    assert_eq!(
        resolution.problems,
        [Problem::Incompatible {
            consumer: "p.a".into(),
            contract: "p.b.rows".into(),
            provider: "p.b".into(),
            provided: 1,
            required: 2,
        }]
    );
    assert!(resolution.problems[0]
        .to_string()
        .contains("needs contract p.b.rows version 2, but p.b provides version 1"));
}

#[test]
fn an_absent_optional_contract_is_recorded_not_an_error() {
    let mut consumer = plugin("p.a", &["p.b"], true);
    consumer.requires[0].optional = true;
    let installed = [consumer, plugin("p.b", &[], false)];
    let resolution = resolve(&installed, &ProjectPlugins::default());
    assert_eq!(
        ids(&resolution),
        ["p.a"],
        "an optional provider is not pulled in"
    );
    assert!(resolution.problems.is_empty());
    assert_eq!(
        resolution.absent_optional,
        [AbsentOptional {
            consumer: "p.a".into(),
            contract: "p.b.rows".into()
        }]
    );
    let with_b = resolve(&installed, &ProjectPlugins::default().with(&["p.b"], true));
    assert!(with_b.absent_optional.is_empty());
}

#[test]
fn two_enabled_providers_of_one_contract_conflict() {
    let mut replacement = plugin("p.b2", &[], true);
    replacement.provides[0].id = "p.b.rows".into();
    let installed = [plugin("p.b", &[], true), replacement];
    let resolution = resolve(&installed, &ProjectPlugins::default());
    assert_eq!(
        resolution.problems,
        [Problem::Conflict {
            contract: "p.b.rows".into(),
            providers: vec!["p.b".into(), "p.b2".into()],
        }]
    );
    // The project picks one by disabling the other.
    let picked = resolve(&installed, &ProjectPlugins::default().with(&["p.b"], false));
    assert!(picked.problems.is_empty());
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

/// A plugin root's build in this test binary: what a library exports.
fn probe_build(p: &mut PluginBuilder) {
    p.add(Probe {
        origin: "probe_library_plugin",
        name: "built",
    });
    // Naming another origin is refused.
    p.add(Probe {
        origin: "someone_else",
        name: "stolen",
    });
}

inventory::submit! {
    InstalledPlugin {
        descriptor: "id = \"p.library\"\nname = \"Library\"\nroot = \"probe_library_plugin\"\n",
        root: "probe_library_plugin",
        build: Some(probe_build),
    }
}

#[test]
fn a_build_registers_under_its_root_and_refuses_another_origin() {
    let built: Vec<&str> = registered::<Probe>().map(|probe| probe.name).collect();
    assert!(built.contains(&"built"), "{built:?}");
    assert!(!built.contains(&"stolen"));
    let (roots, problems) = builds();
    assert!(roots.contains(&"probe_library_plugin"));
    assert!(
        problems.iter().any(|p| p.contains("someone_else")),
        "{problems:?}"
    );
}

#[test]
fn a_library_export_imports_by_category_and_layout() {
    let exported = library::exported();
    // SAFETY: this copy's own export, which lives for the process.
    let imported = unsafe { library::import(&exported) }.unwrap();
    assert_eq!(imported.plugins, ["p.library"]);
    assert!(imported.refused.is_empty(), "{:?}", imported.refused);
    assert!(installed().iter().any(|plugin| plugin.id == "p.library"));

    // A registration built against another layout, and one of a category
    // this engine does not have, are refused by name.
    let plugin = unsafe { *exported.plugins };
    let entry = unsafe { *plugin.entries };
    let wrong = [
        library::FfiEntry {
            fingerprint: entry.fingerprint ^ 1,
            ..entry
        },
        library::FfiEntry {
            category: library::FfiStr {
                ptr: "nowhere.kind".as_ptr(),
                len: "nowhere.kind".len(),
            },
            ..entry
        },
    ];
    let forged_plugin = [library::FfiPlugin {
        entries: wrong.as_ptr(),
        len: wrong.len(),
        ..plugin
    }];
    let forged = library::FfiLibrary {
        plugins: forged_plugin.as_ptr(),
        len: 1,
        ..exported
    };
    // SAFETY: the forged arrays outlive the call; the entries point to a
    // live registration.
    let refused = unsafe { library::import(&forged) }.unwrap().refused;
    assert_eq!(refused.len(), 2, "{refused:?}");
    assert!(refused[0].contains("another layout"), "{refused:?}");
    assert!(refused[1].contains("nowhere.kind"), "{refused:?}");
    // Another export layout is refused whole.
    let other = library::FfiLibrary {
        abi: 99,
        ..exported
    };
    assert!(unsafe { library::import(&other) }.is_err());
}

#[test]
fn a_layout_fingerprint_changes_with_the_type_and_its_version() {
    struct Bumped;
    impl Contribution for Bumped {
        const CATEGORY: &'static str = "test.probe";
        const VERSION: u32 = 2;
        fn origin(&self) -> &'static str {
            ""
        }
        fn id(&self) -> String {
            String::new()
        }
    }
    assert_eq!(
        library::fingerprint::<Probe>(),
        library::fingerprint::<Probe>()
    );
    assert_ne!(
        library::fingerprint::<Probe>(),
        library::fingerprint::<Bumped>()
    );
}
