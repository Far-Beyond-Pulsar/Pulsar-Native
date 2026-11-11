//! The trait proof (Pulsar-Native#1608, T5): a fixture plugin
//! (`tests/fixtures/trait_plugin`, `pulsar.test_traits`) defines a trait and
//! a role and implements the trait with a component; a Blueprint implements
//! it too.
//!
//! - Catalog: the plugin's trait definition, role, native implementer and file
//!   provider are contributions under its id, and its trait uses are declared
//!   by its own `trait` contract.
//! - Game: the project's trait index lists the Blueprint class and the
//!   component as implementers; a script driver spawns the Blueprint class as
//!   the role's entity, and `call_trait` reaches the class's handler and the
//!   component's method alike.
//! - Disabled plugin: the trait, the role and the native implementer are
//!   absent, and the Blueprint's reference to the trait is left unresolved
//!   (the "Missing trait" the Blueprint trait panel shows).

use std::path::Path;
use std::sync::{Arc, Mutex};

use plugin_manager::{CatalogProblem, CatalogSnapshot};
use pulsar_class::ClassRegistry;
use pulsar_game::scripting::{new_runtime, DriverReport, HookOutcome, ScriptDriver};
use pulsar_plugins::{Resolution, Summary};
use pulsar_scenedb::World;
use pulsar_script_vm::{
    Constant, EventDecl, EventRef, Function, Host, Import, Instr, Module, NativeFn, Param,
    Signature, Subscription, SubscriptionScope, Type,
};
use pulsar_traits::{Binding, BindingStatus, RoleSelection, TraitIndex, WorldRoles};
use serde_json::json;
use trait_fixture_plugin::{GreeterComponent, GREETER_ROLE, GREETER_SETTING, GREETER_TRAIT};

// The installed plugins (the Blueprint plugin's implementer source among them)
// and the fixture, linked for their registrations. The fixture's items are
// named above, which links it.
use installed_plugins as _;

const PLUGIN: &str = "pulsar.test_traits";
const ROOT: &str = "trait_fixture_plugin";

/// The Blueprint class `Hero`: its `graph_save.json` (what the Blueprint
/// source claims and scans) and the class folder it compiles to.
const HERO_ASSET: &str = "src/classes/Hero/graph_save.json";
const HERO_DIR: &str = "src/classes/Hero";
const HERO_LOG: &str = "hero:greeted";
const NATIVE_IMPLEMENTER: &str = "component:GreeterComponent";

/// The tests change the process's activated plugins: one at a time.
static ACTIVATION: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    ACTIVATION
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

// ---- the Blueprint class ----------------------------------------------------

/// What compiling the Blueprint `Hero` produces for the trait's `greet`: a
/// handler of the event `Hero.greet` that calls the test native
/// `test::greeted` with a label.
fn hero_module() -> Module {
    let mut module = Module::new("Hero");
    module.imports = vec![Import {
        name: "test::greeted".into(),
        sig: Signature::new([Param::new(Type::Str)], Type::Unit),
    }];
    module.constants.push(Constant::Str(HERO_LOG.into()));
    module.functions.push(Function {
        name: "on_greet".into(),
        exported: false,
        params: vec![],
        ret: Type::Unit,
        registers: vec![Type::Str],
        code: vec![
            Instr::Const { dst: 0, index: 0 },
            Instr::CallNative {
                import: 0,
                args: vec![0],
                dst: None,
            },
            Instr::Return { value: None },
        ],
        debug: None,
    });
    module.events.push(EventDecl {
        name: "Hero.greet".into(),
        fields: vec![],
    });
    module.subscriptions.push(Subscription {
        event: EventRef::Name("Hero.greet".into()),
        handler: 0,
        scope: SubscriptionScope::Self_,
    });
    module
}

/// A project with the class `Hero`: a Blueprint (`graph_save.json`) that
/// declares the trait, and the module it compiled to.
fn hero_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let class = dir.path().join(HERO_DIR);
    std::fs::create_dir_all(class.join("events").join(".build")).unwrap();
    std::fs::write(
        class.join("class.json"),
        json!({ "class_id": "Hero-guid" }).to_string(),
    )
    .unwrap();
    std::fs::write(
        class.join("events/.build/module.json"),
        hero_module().to_json().unwrap(),
    )
    .unwrap();
    std::fs::write(
        class.join("graph_save.json"),
        json!({
            "format_version": 1, "main_graph": {}, "variables": [],
            "blueprint_metadata": { "implemented_trait_ids": [GREETER_TRAIT] },
        })
        .to_string(),
    )
    .unwrap();
    dir
}

type Greeted = Arc<Mutex<Vec<String>>>;

/// Register the native `Hero`'s handler calls; what it logs is returned.
fn install_greeted(driver: &mut ScriptDriver) -> Greeted {
    let greeted = Greeted::default();
    let sink = Arc::clone(&greeted);
    driver
        .runtime_mut()
        .register_native(NativeFn::builder("test::greeted").params(["label"]).build(
            move |_host: &mut Host<'_>, label: String| {
                sink.lock().unwrap().push(label);
            },
        ))
        .unwrap();
    greeted
}

fn logged(greeted: &Greeted) -> Vec<String> {
    greeted.lock().unwrap().clone()
}

/// A driver over the project at `root`, with `Hero` selected for the role.
fn driver_over(root: &Path, index: &TraitIndex) -> (ScriptDriver, Greeted) {
    let mut driver = ScriptDriver::with_parts(
        new_runtime(),
        root,
        ClassRegistry::scan(root),
        Default::default(),
    );
    let greeted = install_greeted(&mut driver);
    let mut selection = RoleSelection::default();
    selection.set(GREETER_ROLE, Some(HERO_ASSET.to_owned()));
    let roles = WorldRoles::resolve(&selection, Some(index));
    assert_eq!(
        roles.get(GREETER_ROLE).map(|binding| binding.status),
        Some(BindingStatus::Valid),
        "Hero implements the role's trait: {:?}",
        roles.diagnostics()
    );
    driver.set_world_roles(roles);
    (driver, greeted)
}

fn without_the_plugin() -> Resolution {
    Resolution {
        enabled: pulsar_plugins::installed()
            .iter()
            .map(|plugin| plugin.id.clone())
            .filter(|id| id != PLUGIN)
            .collect(),
        ..Resolution::default()
    }
}

// ---- the catalog ------------------------------------------------------------

#[test]
fn the_catalog_lists_the_plugins_trait_role_native_implementer_and_provider() {
    let _guard = lock();
    pulsar_plugins::deactivate();
    let catalog = CatalogSnapshot::snapshot();
    let plugin = catalog.plugin(PLUGIN).expect("the fixture is installed");
    assert!(plugin.enabled);
    assert_eq!(plugin.modules, [ROOT]);

    let of = |category: &str| -> Vec<&Summary> {
        plugin
            .contributions
            .iter()
            .filter(|summary| summary.category == category)
            .collect()
    };
    let uses_trait = |summary: &Summary| {
        summary.children.iter().any(|child| {
            child.category == pulsar_traits::sources::TRAIT_USE && child.id == GREETER_TRAIT
        })
    };

    // The trait, read from the file the plugin's folder serves.
    let definitions = of(pulsar_traits::catalog::DEFINITION);
    assert!(
        definitions
            .iter()
            .any(|summary| summary.id == GREETER_TRAIT && summary.origin == ROOT),
        "{definitions:?}"
    );

    // The role, using the trait.
    let roles = of("gameplay.roles");
    let role = roles
        .iter()
        .find(|summary| summary.id == GREETER_ROLE)
        .unwrap_or_else(|| panic!("the role is missing from {roles:?}"));
    assert!(uses_trait(role), "{role:?}");

    // The component, implementing the trait.
    let natives = of("traits.native_implementers");
    let native = natives
        .iter()
        .find(|summary| summary.id == format!("GreeterComponent: {GREETER_TRAIT}"))
        .unwrap_or_else(|| panic!("the native implementer is missing from {natives:?}"));
    assert!(uses_trait(native), "{native:?}");

    // The folder that serves the trait file.
    let providers = of("fs.file_providers");
    assert!(
        providers.iter().any(|summary| summary.id == PLUGIN),
        "{providers:?}"
    );

    // Everything is the plugin's, and its own trait contract covers its use.
    for summary in plugin.contributions.iter().flat_map(Summary::flatten) {
        assert_eq!(summary.origin, ROOT, "{summary:?}");
    }
    let undeclared: Vec<&CatalogProblem> = catalog
        .problems
        .iter()
        .filter(|problem| {
            matches!(
                problem,
                CatalogProblem::UndeclaredTraitUse { .. }
                    | CatalogProblem::UndeclaredTraitDefinition { .. }
            )
        })
        .collect();
    assert!(undeclared.is_empty(), "{undeclared:?}");
}

// ---- the game ----------------------------------------------------------------

#[test]
fn a_blueprint_and_a_component_both_implement_the_plugins_trait_and_dispatch_in_a_game() {
    let _guard = lock();
    pulsar_plugins::deactivate();
    let project = hero_project();
    let root = project.path();

    // What the editor stores when it opens the project: the Blueprint source
    // found Hero, the native source lists the component.
    let index = TraitIndex::rebuild(root).unwrap();
    let hero = index.entry(HERO_ASSET).expect("Hero is indexed");
    assert_eq!(hero.kind, "pulsar.blueprints.blueprint");
    assert_eq!(hero.implemented_trait_ids, [GREETER_TRAIT]);
    assert!(hero.unresolved.is_empty(), "{:?}", hero.unresolved);
    assert_eq!(
        hero.binding,
        Some(Binding::ScriptClass {
            dir: HERO_DIR.into()
        })
    );
    let native = index
        .entry(NATIVE_IMPLEMENTER)
        .expect("the component is a listed implementer");
    assert_eq!(native.implemented_trait_ids, [GREETER_TRAIT]);
    assert_eq!(
        native.binding,
        Some(Binding::Component {
            class: "GreeterComponent".into()
        })
    );
    let implementers: Vec<&str> = index
        .implementers_of(GREETER_TRAIT)
        .into_iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(implementers, [NATIVE_IMPLEMENTER, HERO_ASSET]);
    assert!(
        index
            .problems()
            .iter()
            .all(|problem| !problem.contains("Greeter")),
        "the component conforms to the trait: {:?}",
        index.problems()
    );

    // A level's `world_settings` selects the implementer under the role's key.
    let mut settings = serde_json::Map::new();
    settings.insert(GREETER_SETTING.into(), json!(HERO_ASSET));
    assert_eq!(
        RoleSelection::from_world_settings(&serde_json::Value::Object(settings)).get(GREETER_ROLE),
        Some(HERO_ASSET)
    );

    // The Blueprint class as the role's entity.
    let (mut driver, greeted) = driver_over(root, &index);
    let mut world = World::new();
    let mut report = DriverReport::default();
    for _ in 0..2 {
        let frame = driver.run_frame(&mut world, 0.0);
        report.failures.extend(frame.failures);
        report.script_errors.extend(frame.script_errors);
    }
    let hero_entity = driver
        .role_entity(GREETER_ROLE)
        .expect("the role's entity was spawned");
    // Spawned at the end of the first frame, running from the second.
    assert!(
        driver.instance_of(hero_entity).is_some(),
        "the role's script is running"
    );
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(logged(&greeted).is_empty(), "nothing called it yet");

    let outcome = driver.call_trait(
        &mut world,
        hero_entity,
        GREETER_TRAIT,
        "greet",
        &[],
        &mut report,
    );
    assert_eq!(outcome, HookOutcome::Called);
    assert_eq!(logged(&greeted), [HERO_LOG], "the class's handler ran");

    // The component, attached to an object, through the same dispatcher: the
    // trait's `greet` is the component's by-name `greet`.
    let owner = world.spawn();
    let instance =
        pulsar_world_registry::attach_value(&mut world, owner, GreeterComponent::default())
            .unwrap();
    assert_eq!(
        world.get::<GreeterComponent>(instance).unwrap().greetings,
        0
    );
    let outcome = driver.call_trait(&mut world, owner, GREETER_TRAIT, "greet", &[], &mut report);
    assert_eq!(outcome, HookOutcome::Called);
    assert_eq!(
        world.get::<GreeterComponent>(instance).unwrap().greetings,
        1
    );
    assert_eq!(
        logged(&greeted),
        [HERO_LOG],
        "no script ran for the component"
    );

    // An object with neither does not honour the trait.
    let bare = world.spawn();
    assert_eq!(
        driver.call_trait(&mut world, bare, GREETER_TRAIT, "greet", &[], &mut report),
        HookOutcome::NotRunning
    );
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(
        report.script_errors.is_empty(),
        "{:?}",
        report.script_errors
    );
}

// ---- the plugin disabled -------------------------------------------------------

#[test]
fn disabling_the_plugin_leaves_missing_trait_diagnostics_and_enabling_restores_it() {
    let _guard = lock();
    pulsar_plugins::deactivate();
    let project = hero_project();
    let root = project.path();

    pulsar_plugins::activate(&without_the_plugin());
    let index = TraitIndex::rebuild(root).unwrap();
    let catalog = CatalogSnapshot::snapshot();
    let role_while_disabled = pulsar_traits::roles::role(GREETER_ROLE).is_some();
    pulsar_plugins::deactivate();

    // Hero stays an implementer, with the trait it declares unresolved: the
    // "Missing trait" the Blueprint trait panel shows.
    let hero = index.entry(HERO_ASSET).expect("Hero is still indexed");
    assert_eq!(hero.unresolved, [GREETER_TRAIT]);
    assert!(
        !hero
            .implemented_trait_ids
            .iter()
            .any(|id| id == GREETER_TRAIT),
        "{:?}",
        hero.implemented_trait_ids
    );
    assert!(index.resolve_trait(GREETER_TRAIT).is_none());
    assert!(!index
        .traits()
        .iter()
        .any(|record| record.id == GREETER_TRAIT));

    // The native implementer and the role are gone.
    assert!(index.entry(NATIVE_IMPLEMENTER).is_none());
    assert!(!role_while_disabled, "the role is absent");

    // The catalog still lists the plugin and what it brings, to diagnose.
    let plugin = catalog.plugin(PLUGIN).expect("still installed");
    assert!(!plugin.enabled);
    assert!(
        plugin
            .contributions
            .iter()
            .any(|summary| summary.category == "gameplay.roles" && summary.id == GREETER_ROLE),
        "{:?}",
        plugin.contributions
    );

    // Enabled again: the role is back, and so are the trait and the
    // implementers, with nothing unresolved.
    assert!(pulsar_traits::roles::role(GREETER_ROLE).is_some());
    let index = TraitIndex::rebuild(root).unwrap();
    let hero = index.entry(HERO_ASSET).unwrap();
    assert_eq!(hero.implemented_trait_ids, [GREETER_TRAIT]);
    assert!(hero.unresolved.is_empty(), "{:?}", hero.unresolved);
    assert!(index.entry(NATIVE_IMPLEMENTER).is_some());
}
