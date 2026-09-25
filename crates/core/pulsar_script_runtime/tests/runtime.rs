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
    Function { name: name.into(), exported: true, params, ret: Type::Unit, registers, code, debug: None }
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

    // #862: a reload whose code keeps the waiting function's layout keeps
    // the waiting call, and it finishes in the NEW code (here the second
    // constant changed from "b" to "B").
    rt.spawn("b", "Latent", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    assert_eq!(rt.waiting_calls("b"), 1);
    let mut v2 = m.clone();
    v2.constants[1] = Constant::Str("B".into());
    let report = rt.reload_class(v2.clone()).unwrap();
    assert_eq!((report.kept, report.dropped.len()), (1, 0));
    assert_eq!(rt.waiting_calls("b"), 1);
    rt.tick_all(&mut world, 1.0);
    assert_eq!(rt.variable("b", "log"), Some(&Value::from("aB")), "resumed in the new code");

    // A reload that changes the waiting function's instruction count drops
    // the call and names it.
    rt.spawn("c", "Latent", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    assert_eq!(rt.waiting_calls("c"), 1);
    let mut v3 = v2;
    v3.functions[0].code.insert(0, Move { dst: 0, src: 0 });
    let report = rt.reload_class(v3).unwrap();
    assert_eq!(rt.waiting_calls("c"), 0);
    assert_eq!(report.dropped.len(), 1);
    assert_eq!(report.dropped[0].object_id, "c");
    assert_eq!(report.dropped[0].function, "begin_play");
    assert!(report.dropped[0].reason.contains("instruction count"), "{}", report.dropped[0].reason);
}

/// #854: a runtime error names the class, the instance, the function and
/// the graph node the function's debug info maps the failing pc to; a link
/// error names the node that calls the missing native.
#[test]
fn errors_carry_class_function_and_node() {
    use pulsar_script_vm::{DebugInfo, SourceLoc};
    let mut rt = runtime();
    let mut world = World::new();
    let mut m = Module::new("Divider");
    m.constants = vec![Constant::Int(1), Constant::Int(0)];
    let mut tick = function("tick", vec![Type::Float], vec![Type::Int, Type::Int], vec![
        Const { dst: 1, index: 0 },
        Const { dst: 2, index: 1 },
        Binary { op: BinOp::Div, dst: 1, a: 1, b: 2 },
        Return { value: None },
    ]);
    let mut debug = DebugInfo::default();
    let consts = SourceLoc::node("graph_save.json", "literal_1");
    let divide = SourceLoc::node("graph_save.json", "divide_7");
    debug.record(0, &consts);
    debug.record(1, &consts);
    debug.record(2, &divide);
    tick.debug = Some(debug);
    m.functions = vec![tick];
    rt.load_class(m.clone()).unwrap();
    rt.spawn("d", "Divider", None, &[]).unwrap();
    rt.dispatch_pending_begin_play(&mut world);
    let errors = rt.tick_all(&mut world, 0.1);
    assert_eq!(errors.len(), 1);
    let details = errors[0].details();
    assert_eq!(details.class.as_deref(), Some("Divider"));
    assert_eq!(details.object_id.as_deref(), Some("d"));
    assert_eq!(details.function.as_deref(), Some("tick"));
    assert_eq!(details.pc, Some(2));
    assert_eq!(details.location.as_ref().map(|l| l.node.as_str()), Some("divide_7"));
    let text = errors[0].to_string();
    assert!(text.contains("Divider") && text.contains("node divide_7"), "{text}");

    // Link error: the native the `tick` of a new class calls is missing.
    let mut broken = Module::new("Broken");
    broken.imports = vec![Import { name: "nope::missing".into(), sig: Signature::new([], Type::Unit) }];
    let mut f = function("tick", vec![Type::Float], vec![], vec![
        CallNative { import: 0, args: vec![], dst: None },
        Return { value: None },
    ]);
    let mut debug = DebugInfo::default();
    debug.record(0, &SourceLoc::node("graph_save.json", "call_3"));
    f.debug = Some(debug);
    broken.functions = vec![f];
    let error = rt.load_class(broken).unwrap_err();
    let details = error.details();
    assert_eq!(details.class.as_deref(), Some("Broken"));
    assert_eq!(details.function.as_deref(), Some("tick"));
    assert_eq!(details.location.map(|l| l.node), Some("call_3".to_owned()));
    assert!(error.to_string().contains("node call_3"), "{error}");
}

// ---- events (#924) ----------------------------------------------------------

mod events {
    use super::*;
    use std::sync::{Arc, Mutex};

    use pulsar_script_runtime::EventHost;
    use pulsar_script_vm::{
        EventCatalog, EventDecl, EventField, EventRef, EventSignature, EventSink, EventTarget, FuncId,
        Subscription, SubscriptionScope,
    };

    #[derive(Default)]
    struct Hub {
        declared: Mutex<Vec<EventSignature>>,
        emitted: Mutex<Vec<(EventTarget, String, Vec<Value>)>>,
    }

    impl EventSink for Hub {
        fn emit(&self, target: EventTarget, name: &str, fields: &[Value]) -> Result<(), String> {
            self.emitted.lock().unwrap().push((target, name.into(), fields.to_vec()));
            Ok(())
        }
    }

    impl EventCatalog for Hub {
        fn event_by_name(&self, name: &str) -> Option<EventSignature> {
            self.declared.lock().unwrap().iter().find(|e| e.name == name).cloned()
        }
        fn event_by_id(&self, id: u64) -> Option<EventSignature> {
            self.declared.lock().unwrap().iter().find(|e| e.id == id).cloned()
        }
    }

    impl EventHost for Hub {
        fn declare(&self, _class: &str, decl: &EventDecl) -> Result<(), String> {
            let mut declared = self.declared.lock().unwrap();
            let id = declared.len() as u64 + 1;
            if let Some(existing) = declared.iter().find(|e| e.name == decl.name) {
                return if existing.fields == decl.fields { Ok(()) } else { Err("conflict".into()) };
            }
            declared.push(EventSignature { id, name: decl.name.clone(), fields: decl.fields.clone() });
            Ok(())
        }
    }

    /// Declares `Ping(n: int)`, handles it with a non-exported `on_ping`
    /// that adds `n` to `total`, and emits `Ping(5)` from `send`.
    fn pinger(name: &str, other_event: Option<&str>) -> Module {
        let mut m = Module::new(name);
        m.variables = vec![Variable { name: "total".into(), ty: Type::Int, default: None }];
        m.constants = vec![Constant::Str("Ping".into()), Constant::Int(5)];
        m.imports = vec![Import {
            name: "event::emit".into(),
            sig: Signature::new([Param::new(Type::Str), Param::new(Type::Int)], Type::Unit),
        }];
        let mut on_ping = function("on_ping", vec![Type::Int], vec![Type::Int], vec![
            LoadVar { dst: 1, var: 0 },
            Binary { op: BinOp::Add, dst: 1, a: 1, b: 0 },
            StoreVar { var: 0, src: 1 },
            Return { value: None },
        ]);
        on_ping.exported = false;
        m.functions = vec![
            on_ping,
            function("send", vec![], vec![Type::Str, Type::Int], vec![
                Const { dst: 0, index: 0 },
                Const { dst: 1, index: 1 },
                CallNative { import: 0, args: vec![0, 1], dst: None },
                Return { value: None },
            ]),
        ];
        m.events = vec![EventDecl { name: "Ping".into(), fields: vec![EventField::new("n", Type::Int)] }];
        m.subscriptions = vec![Subscription {
            event: EventRef::Name(other_event.unwrap_or("Ping").into()),
            handler: 0,
            scope: SubscriptionScope::Self_,
        }];
        m
    }

    #[test]
    fn classes_declare_link_against_and_publish_through_the_host() {
        let hub = Arc::new(Hub::default());
        let mut rt = runtime();
        rt.set_event_host(Some(hub.clone() as Arc<dyn EventHost>));
        let mut world = World::new();
        rt.load_class(pinger("Pinger", None)).unwrap();
        assert_eq!(hub.event_by_name("Ping").unwrap().fields.len(), 1, "declared before linking");
        let subs = rt.subscriptions("Pinger").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].event_id, Some(1), "linked against the host's catalog");
        assert_eq!(subs[0].handler, FuncId(0));

        rt.spawn("p", "Pinger", None, &[]).unwrap();
        rt.send_event("p", "send", &[], &mut world).unwrap();
        let emitted = hub.emitted.lock().unwrap().clone();
        assert_eq!(emitted, vec![(EventTarget::Global, "Ping".to_owned(), vec![Value::Int(5)])]);

        // Handlers need not be exported.
        rt.call_function("p", FuncId(0), &[Value::Int(3)], &mut world).unwrap();
        assert_eq!(rt.variable("p", "total"), Some(&Value::Int(3)));

        // A class subscribing to an event nobody declared does not link.
        let err = rt.load_class(pinger("Other", Some("Nope"))).unwrap_err();
        assert!(err.to_string().contains("Nope"), "{err}");
    }
}
