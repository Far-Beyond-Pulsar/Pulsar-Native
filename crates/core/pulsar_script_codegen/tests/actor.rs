use pulsar_script_codegen::actor::{class_files, generate_actor, pascal_case, snake_case, ComponentSpec, ExportError};
use pulsar_script_codegen::{generate, CodegenError};
use pulsar_script_vm::{
    EventDecl, EventField, EventRef, Function, Instr, Module, Subscription, SubscriptionScope, Type,
};

fn empty_class(name: &str) -> Module {
    let mut m = Module::new(name);
    m.functions.push(Function {
        name: "begin_play".into(),
        exported: true,
        params: vec![],
        ret: Type::Unit,
        registers: vec![],
        code: vec![Instr::Return { value: None }],
        debug: None,
    });
    m
}

#[test]
fn names_follow_the_project_conventions() {
    assert_eq!(snake_case("PlayerController"), "player_controller");
    assert_eq!(snake_case("my-class name"), "my_class_name");
    assert_eq!(pascal_case("player_controller"), "PlayerController");
    assert_eq!(pascal_case("my-class name"), "MyClassName");
}

#[test]
fn the_actor_forwards_the_pinned_time_free_callbacks() {
    let source = generate_actor("PlayerController", &empty_class("PlayerController"), &[]).unwrap();
    assert!(source.contains("pub struct PlayerController {"));
    assert!(source.contains("fn begin_play(&mut self, _entity: Entity, _world: &mut World)"));
    assert!(source.contains("fn tick(&mut self, _entity: Entity, _world: &mut World)"));
    assert!(source.contains("fn end_play(&mut self, _entity: Entity, _world: &mut World)"));
    assert!(source.contains("impl Actor for PlayerController"));
    assert!(!source.contains("GameTime"));
    // State is per actor; nothing is process-global.
    assert!(!source.contains("thread_local") && !source.contains("static mut") && !source.contains("lazy_static"));
    assert!(!source.contains("thread::sleep"), "no node may block the game thread");
}

#[test]
fn enabled_prefab_components_are_hydrated_and_disabled_ones_are_not() {
    let components = [
        ComponentSpec { class_name: "RigidbodyComponent".into(), property_defaults_json: r#"{"mass": 2.0}"#.into(), enabled: true },
        ComponentSpec { class_name: "LightComponent".into(), property_defaults_json: "{}".into(), enabled: false },
    ];
    let source = generate_actor("Crate", &empty_class("Crate"), &components).unwrap();
    assert!(source.contains("__init_components(entity: Entity, world: &mut World)"));
    assert!(source.contains("hydrate_world_component_for_class("));
    assert!(source.contains("RigidbodyComponent"));
    assert!(source.contains(r#"{\"mass\": 2.0}"#), "defaults are embedded as an escaped literal");
    assert!(!source.contains("LightComponent"));

    let none = generate_actor("Crate", &empty_class("Crate"), &[]).unwrap();
    assert!(!none.contains("__init_components"));
}

#[test]
fn classes_with_custom_events_are_refused_not_silently_degraded() {
    let mut with_events = empty_class("Door");
    with_events.events = vec![EventDecl { name: "Door.Open".into(), fields: vec![EventField { name: "by".into(), ty: Type::Entity }] }];
    let error = generate_actor("Door", &with_events, &[]).unwrap_err();
    assert!(matches!(error, ExportError::Unsupported(_)));
    assert!(error.to_string().contains("Door.Open"));

    let mut subscribed = empty_class("Door");
    subscribed.subscriptions =
        vec![Subscription { event: EventRef::Name("Hit".into()), handler: 0, scope: SubscriptionScope::Global }];
    let error = generate_actor("Door", &subscribed, &[]).unwrap_err();
    assert!(matches!(error, ExportError::Unsupported(_)));
    assert!(error.to_string().contains("subscription #0 for event `Hit`"));
}

#[test]
fn component_event_subscriptions_report_the_unavailable_binding_context() {
    let mut subscribed = empty_class("TerrainListener");
    subscribed.variables.push(pulsar_script_vm::Variable {
        name: "terrain".into(),
        ty: Type::component("VoxelTerrainComponent"),
        default: None,
        id: Some("bp-slot:terrain-instance".into()),
    });
    subscribed.subscriptions = vec![Subscription {
        event: EventRef::Name("VoxelTerrainComponent.BlockBroken".into()),
        handler: 0,
        scope: SubscriptionScope::Component(0),
    }];

    let error = generate_actor("TerrainListener", &subscribed, &[]).unwrap_err();
    assert!(matches!(error, ExportError::Unsupported(_)));
    let message = error.to_string();
    assert!(message.contains("subscription #0"));
    assert!(message.contains("VoxelTerrainComponent.BlockBroken"));
    assert!(message.contains("component-reference variable 0 (`terrain`: VoxelTerrainComponent&)"));
    assert!(message.contains("Use the VM compile target"));
}

#[test]
fn a_module_that_does_not_verify_generates_nothing() {
    let mut bad = empty_class("Bad");
    bad.functions[0].code = vec![Instr::Jump { target: 99 }];
    assert!(matches!(generate(&bad), Err(CodegenError::Verify(_))));
    assert!(generate_actor("Bad", &bad, &[]).is_err());
}

#[test]
fn generated_code_carries_data_not_bytecode() {
    let source = generate(&empty_class("Plain")).unwrap();
    // The module data is embedded without its instruction streams: what runs
    // is the compiled step function.
    assert!(source.contains("fn step_0("));
    assert!(source.contains("\"code\": []") || source.contains("\"code\":[]"), "instruction streams are stripped from the data");
    assert!(!source.contains("Interpreter") && !source.contains("Vm::new"));
}

#[test]
fn the_class_tree_has_the_layout_the_project_builder_scans() {
    let files = class_files("Layout Probe", &empty_class("Layout Probe"), &[]).unwrap();
    let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        ["src/classes/layout_probe/mod.rs", "src/classes/layout_probe/events/mod.rs", "src/classes/layout_probe/events/events.rs"]
    );
}
