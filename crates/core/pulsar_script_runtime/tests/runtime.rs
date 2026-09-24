//! Lifecycle, instances, variables, events and hot reload.

use std::collections::HashMap;

use pulsar_scenedb::World;
use pulsar_script_runtime::{RuntimeError, ScriptRuntime};
use pulsar_script_vm::{
    BinOp, Constant, Function, Import, Instr, Module, NativeFn, NativeRegistry, Param, Signature,
    Type, Value, Variable,
};

use Instr::*;

fn runtime() -> ScriptRuntime {
    ScriptRuntime::new(std::env::temp_dir().join(format!("pulsar_script_runtime_{}", std::process::id())))
}

fn function(name: &str, params: Vec<Type>, extra: Vec<Type>, code: Vec<Instr>) -> Function {
    let mut registers = params.clone();
    registers.extend(extra);
    Function { name: name.into(), exported: true, params, ret: Type::Unit, registers, code }
}

/// `elapsed` accumulates delta time, `ticks` counts ticks, `started` is set
/// by begin_play and cleared by end_play. `add(n)` adds to `ticks`.
fn counter(name: &str) -> Module {
    let mut m = Module::new(name);
    m.variables = vec![
        Variable { name: "elapsed".into(), ty: Type::Float, default: None },
        Variable { name: "ticks".into(), ty: Type::Int, default: None },
        Variable { name: "started".into(), ty: Type::Bool, default: None },
        Variable { name: "step".into(), ty: Type::Int, default: Some(Constant::Int(1)) },
    ];
    m.constants = vec![Constant::Bool(true), Constant::Bool(false)];
    m.functions = vec![
        function("begin_play", vec![], vec![Type::Bool], vec![
            Const { dst: 0, index: 0 },
            StoreVar { var: 2, src: 0 },
            Return { value: None },
        ]),
        function("tick", vec![Type::Float], vec![Type::Float, Type::Int, Type::Int], vec![
            LoadVar { dst: 1, var: 0 },
            Binary { op: BinOp::Add, dst: 1, a: 1, b: 0 },
            StoreVar { var: 0, src: 1 },
            LoadVar { dst: 2, var: 1 },
            LoadVar { dst: 3, var: 3 },
            Binary { op: BinOp::Add, dst: 2, a: 2, b: 3 },
            StoreVar { var: 1, src: 2 },
            Return { value: None },
        ]),
        function("end_play", vec![], vec![Type::Bool], vec![
            Const { dst: 0, index: 1 },
            StoreVar { var: 2, src: 0 },
            Return { value: None },
        ]),
        function("add", vec![Type::Int], vec![Type::Int], vec![
            LoadVar { dst: 1, var: 1 },
            Binary { op: BinOp::Add, dst: 1, a: 1, b: 0 },
            StoreVar { var: 1, src: 1 },
            Return { value: None },
        ]),
    ];
    m
}

#[test]
fn lifecycle_runs_in_order_with_delta_time() {
    let mut rt = runtime();
    let mut world = World::new();
    rt.load_class(counter("Counter")).unwrap();
    rt.spawn("a", "Counter", None, &[]).unwrap();

    // Not begun yet: tick skips it.
    assert!(rt.tick_all(&mut world, 0.5).is_empty());
    assert_eq!(rt.variable("a", "ticks"), Some(&Value::Int(0)));

    assert!(rt.dispatch_pending_begin_play(&mut world).is_empty());
    assert_eq!(rt.variable("a", "started"), Some(&Value::Bool(true)));
    rt.tick_all(&mut world, 0.25);
    rt.tick_all(&mut world, 0.5);
    assert_eq!(rt.variable("a", "elapsed"), Some(&Value::Float(0.75)));
    assert_eq!(rt.variable("a", "ticks"), Some(&Value::Int(2)));

    rt.end_play_all(&mut world);
    assert_eq!(rt.variable("a", "started"), Some(&Value::Bool(false)));
}

#[test]
fn instances_are_isolated_and_take_overrides() {
    let mut rt = runtime();
    let mut world = World::new();
    rt.load_class(counter("Counter")).unwrap();
    rt.spawn("a", "Counter", None, &[]).unwrap();
    rt.spawn("b", "Counter", None, &[("step".into(), Value::Int(10))]).unwrap();
    let json: HashMap<String, serde_json::Value> = [("step".to_string(), serde_json::json!(100))].into();
    rt.spawn_with_json("c", "Counter", None, &json).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    rt.tick_all(&mut world, 1.0);
    assert_eq!(rt.variable("a", "ticks"), Some(&Value::Int(1)));
    assert_eq!(rt.variable("b", "ticks"), Some(&Value::Int(10)));
    assert_eq!(rt.variable("c", "ticks"), Some(&Value::Int(100)));

    assert!(matches!(
        rt.spawn("d", "Counter", None, &[("step".into(), Value::Float(1.0))]),
        Err(RuntimeError::BadVariable { .. })
    ));
    // JSON overrides (level files) skip variables that no longer exist.
    let stale: HashMap<String, serde_json::Value> = [("gone".to_string(), serde_json::json!(1))].into();
    rt.spawn_with_json("e", "Counter", None, &stale).unwrap();
    assert!(matches!(rt.spawn("a", "Counter", None, &[]), Err(RuntimeError::DuplicateInstance(_))));
    assert!(matches!(rt.spawn("x", "Nope", None, &[]), Err(RuntimeError::UnknownClass(_))));
}

#[test]
fn custom_events_and_despawn() {
    let mut rt = runtime();
    let mut world = World::new();
    rt.load_class(counter("Counter")).unwrap();
    rt.spawn("a", "Counter", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    rt.send_event("a", "add", &[Value::Int(5)], &mut world).unwrap();
    assert_eq!(rt.variable("a", "ticks"), Some(&Value::Int(5)));
    assert!(matches!(
        rt.send_event("a", "nope", &[], &mut world),
        Err(RuntimeError::UnknownEvent { .. })
    ));
    assert!(matches!(
        rt.send_event("a", "add", &[Value::Float(1.0)], &mut world),
        Err(RuntimeError::Script { .. })
    ));

    rt.despawn("a", &mut world).unwrap();
    assert!(rt.instance_ids().is_empty());
    assert!(rt.variable("a", "ticks").is_none());
}

#[test]
fn reload_keeps_matching_variables() {
    let mut rt = runtime();
    let mut world = World::new();
    rt.load_class(counter("Counter")).unwrap();
    rt.spawn("a", "Counter", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    rt.tick_all(&mut world, 2.0);

    // New version: `ticks` retyped to float, `elapsed` kept, a new variable.
    let mut v2 = counter("Counter");
    v2.variables[1].ty = Type::Float;
    v2.variables.push(Variable { name: "fresh".into(), ty: Type::Str, default: Some(Constant::Str("new".into())) });
    v2.functions[1].code = vec![Return { value: None }];
    v2.functions[3].registers[1] = Type::Float;
    v2.functions[3].code = vec![Return { value: None }];
    rt.reload_class(v2).unwrap();

    assert_eq!(rt.variable("a", "elapsed"), Some(&Value::Float(2.0)));
    assert_eq!(rt.variable("a", "ticks"), Some(&Value::Float(0.0)));
    assert_eq!(rt.variable("a", "fresh"), Some(&Value::from("new")));
    assert_eq!(rt.variable("a", "started"), Some(&Value::Bool(true)));

    // A broken reload changes nothing.
    let mut broken = counter("Counter");
    broken.functions[0].code.clear();
    assert!(rt.reload_class(broken).is_err());
    assert_eq!(rt.variable("a", "fresh"), Some(&Value::from("new")));
}

#[test]
fn lifecycle_signatures_are_checked() {
    let mut rt = runtime();
    let mut m = Module::new("Bad");
    m.functions = vec![function("tick", vec![], vec![], vec![Return { value: None }])];
    assert!(matches!(rt.load_class(m), Err(RuntimeError::BadEntryPoint { name: "tick", .. })));
}

#[test]
fn unbound_instances_cannot_reach_components() {
    let mut rt = runtime();
    let mut world = World::new();
    let mut m = Module::new("Selfish");
    m.imports = vec![Import {
        name: "entity::is_alive".into(),
        sig: Signature::new([Param::new(Type::Entity)], Type::Bool),
    }];
    m.variables = vec![Variable { name: "alive".into(), ty: Type::Bool, default: None }];
    m.functions = vec![function("begin_play", vec![], vec![Type::Entity, Type::Bool], vec![
        SelfEntity { dst: 0 },
        CallNative { import: 0, args: vec![0], dst: Some(1) },
        StoreVar { var: 0, src: 1 },
        Return { value: None },
    ])];
    rt.load_class(m).unwrap();
    let e = world.spawn();
    rt.spawn("bound", "Selfish", Some(e), &[]).unwrap();
    rt.spawn("unbound", "Selfish", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    assert_eq!(rt.variable("bound", "alive"), Some(&Value::Bool(true)));
    assert_eq!(rt.variable("unbound", "alive"), Some(&Value::Bool(false)));
}

#[test]
fn new_natives_relink_classes() {
    let mut rt = ScriptRuntime::with_natives(NativeRegistry::new(), std::env::temp_dir());
    let mut world = World::new();
    let mut m = Module::new("UsesLater");
    m.imports = vec![Import { name: "game::answer".into(), sig: Signature::new([], Type::Int) }];
    m.variables = vec![Variable { name: "v".into(), ty: Type::Int, default: None }];
    m.functions = vec![function("begin_play", vec![], vec![Type::Int], vec![
        CallNative { import: 0, args: vec![], dst: Some(0) },
        StoreVar { var: 0, src: 0 },
        Return { value: None },
    ])];
    // Cannot load before the native exists...
    assert!(matches!(rt.load_class(m.clone()), Err(RuntimeError::Link { .. })));
    let report = rt.register_native(NativeFn::builder("game::answer").build(|| 42i64)).unwrap();
    assert!(report.failed.is_empty());
    rt.load_class(m).unwrap();
    rt.spawn("a", "UsesLater", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    assert_eq!(rt.variable("a", "v"), Some(&Value::Int(42)));
}

#[test]
fn runaway_scripts_are_stopped_per_instance() {
    let mut rt = runtime();
    let mut world = World::new();
    let mut spin = Module::new("Spin");
    spin.functions = vec![function("tick", vec![Type::Float], vec![], vec![Jump { target: 0 }])];
    rt.load_class(spin).unwrap();
    rt.load_class(counter("Counter")).unwrap();
    rt.budget = 1000;
    rt.spawn("spin", "Spin", None, &[]).unwrap();
    rt.spawn("ok", "Counter", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    let errors = rt.tick_all(&mut world, 1.0);
    assert_eq!(errors.len(), 1);
    assert_eq!(rt.variable("ok", "ticks"), Some(&Value::Int(1)));
}

#[test]
fn waiting_events_resume_after_game_time_passes() {
    let mut rt = runtime();
    let mut world = World::new();
    let mut m = Module::new("Latent");
    m.variables = vec![Variable { name: "log".into(), ty: Type::Str, default: None }];
    m.constants = vec![Constant::Str("a".into()), Constant::Str("b".into()), Constant::Float(1.0)];
    // begin_play: log += "a"; wait 1s; log += "b"
    m.functions = vec![function("begin_play", vec![], vec![Type::Str, Type::Str, Type::Float], vec![
        LoadVar { dst: 0, var: 0 },
        Const { dst: 1, index: 0 },
        Binary { op: BinOp::Add, dst: 0, a: 0, b: 1 },
        StoreVar { var: 0, src: 0 },
        Const { dst: 2, index: 2 },
        Wait { seconds: 2 },
        LoadVar { dst: 0, var: 0 },
        Const { dst: 1, index: 1 },
        Binary { op: BinOp::Add, dst: 0, a: 0, b: 1 },
        StoreVar { var: 0, src: 0 },
        Return { value: None },
    ])];
    rt.load_class(m.clone()).unwrap();
    rt.spawn("a", "Latent", None, &[]).unwrap();
    assert!(rt.dispatch_pending_begin_play(&mut world).is_empty());
    assert_eq!(rt.variable("a", "log"), Some(&Value::from("a")));
    assert_eq!(rt.waiting_calls("a"), 1);

    rt.tick_all(&mut world, 0.5);
    assert_eq!(rt.variable("a", "log"), Some(&Value::from("a")));
    rt.tick_all(&mut world, 0.6);
    assert_eq!(rt.variable("a", "log"), Some(&Value::from("ab")));
    assert_eq!(rt.waiting_calls("a"), 0);
    assert!((rt.time() - 1.1).abs() < 1e-9);

    // Reloading drops calls suspended in the old code.
    rt.spawn("b", "Latent", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    assert_eq!(rt.waiting_calls("b"), 1);
    rt.reload_class(m).unwrap();
    assert_eq!(rt.waiting_calls("b"), 0);
}
