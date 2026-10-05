//! Latent actions: waiting for frames, a condition or an event, and
//! timers (set, repeat, retrigger, clear).

use pulsar_scenedb::World;
use pulsar_script_runtime::ScriptRuntime;
use pulsar_script_vm::{
    BinOp, Constant, Function, Import, Instr, Module, Param, Signature, Type, Value, Variable,
};

use Instr::*;

const TICK: f64 = 0.5;

fn runtime() -> ScriptRuntime {
    ScriptRuntime::new(std::env::temp_dir().join(format!("pulsar_script_latent_{}", std::process::id())))
}

fn function(name: &str, params: Vec<Type>, ret: Type, extra: Vec<Type>, code: Vec<Instr>) -> Function {
    let mut registers = params.clone();
    registers.extend(extra);
    Function { name: name.into(), exported: true, params, ret, registers, code, debug: None }
}

fn import(name: &str, params: Vec<Type>, ret: Type) -> Import {
    Import { name: name.into(), sig: Signature::new(params.into_iter().map(Param::new), ret) }
}

/// `go` sets `state = 1`, runs `before` (the latent call), then sets
/// `state = 2`; `fire` bumps `fired`; `is_ready` reads `flag`.
fn class(imports: Vec<Import>, constants: Vec<Constant>, before: Vec<Instr>, extra: Vec<Type>) -> Module {
    let mut m = Module::new("Actor");
    m.variables = vec![
        Variable { name: "state".into(), ty: Type::Int, default: None, id: None },
        Variable { name: "flag".into(), ty: Type::Bool, default: None, id: None },
        Variable { name: "fired".into(), ty: Type::Int, default: None, id: None },
    ];
    m.imports = imports;
    // constants 0 and 1 are the ints 1 and 2; the caller's follow.
    let mut all = vec![Constant::Int(1), Constant::Int(2)];
    all.extend(constants);
    m.constants = all;
    let mut go = vec![Const { dst: 0, index: 0 }, StoreVar { var: 0, src: 0 }];
    go.extend(before);
    go.extend([Const { dst: 0, index: 1 }, StoreVar { var: 0, src: 0 }, Return { value: None }]);
    let mut registers = vec![Type::Int];
    registers.extend(extra);
    m.functions = vec![
        function("go", vec![], Type::Unit, registers, go),
        // flag
        function("is_ready", vec![], Type::Bool, vec![Type::Bool], vec![LoadVar { dst: 0, var: 1 }, Return { value: Some(0) }]),
        // fired += 1
        function(
            "fire",
            vec![],
            Type::Unit,
            vec![Type::Int, Type::Int],
            vec![
                LoadVar { dst: 0, var: 2 },
                Const { dst: 1, index: 0 },
                Binary { op: BinOp::Add, dst: 0, a: 0, b: 1 },
                StoreVar { var: 2, src: 0 },
                Return { value: None },
            ],
        ),
        function("on_ping", vec![], Type::Unit, vec![], vec![Return { value: None }]),
    ];
    m
}

fn start(module: Module) -> (ScriptRuntime, World) {
    let mut rt = runtime();
    rt.load_class(module).unwrap();
    rt.spawn("a", "Actor", None, &[]).unwrap();
    let mut world = World::new();
    assert!(rt.dispatch_pending_begin_play(&mut world).is_empty());
    (rt, world)
}

fn state(rt: &ScriptRuntime) -> i64 {
    rt.variable("a", "state").and_then(Value::as_int).unwrap()
}

fn fired(rt: &ScriptRuntime) -> i64 {
    rt.variable("a", "fired").and_then(Value::as_int).unwrap()
}

fn tick(rt: &mut ScriptRuntime, world: &mut World, times: usize) {
    for _ in 0..times {
        let errors = rt.tick_all(world, TICK);
        assert!(errors.is_empty(), "{errors:?}");
    }
}

#[test]
fn wait_frames_resumes_on_the_nth_tick_and_never_the_same_one() {
    let (mut rt, mut world) = start(class(
        vec![import("wait::frames", vec![Type::Int], Type::Unit)],
        vec![Constant::Int(3)],
        vec![Const { dst: 0, index: 2 }, CallNative { import: 0, args: vec![0], dst: None }],
        vec![],
    ));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    assert_eq!(state(&rt), 1, "ran up to the wait");
    assert_eq!(rt.waiting_calls("a"), 1);
    tick(&mut rt, &mut world, 2);
    assert_eq!(state(&rt), 1, "two of three frames");
    tick(&mut rt, &mut world, 1);
    assert_eq!(state(&rt), 2, "resumed on the third");
    assert_eq!(rt.waiting_calls("a"), 0);
}

#[test]
fn wait_next_tick_resumes_on_the_next_tick() {
    let (mut rt, mut world) = start(class(
        vec![import("wait::next_tick", vec![], Type::Unit)],
        vec![],
        vec![CallNative { import: 0, args: vec![], dst: None }],
        vec![],
    ));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    assert_eq!(state(&rt), 1);
    tick(&mut rt, &mut world, 1);
    assert_eq!(state(&rt), 2);
}

#[test]
fn wait_until_polls_an_exported_predicate_each_tick() {
    let (mut rt, mut world) = start(class(
        vec![import("wait::until", vec![Type::Str], Type::Unit)],
        vec![Constant::Str("is_ready".into())],
        vec![Const { dst: 1, index: 2 }, CallNative { import: 0, args: vec![1], dst: None }],
        vec![Type::Str],
    ));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    tick(&mut rt, &mut world, 3);
    assert_eq!(state(&rt), 1, "the condition does not hold yet");
    rt.set_variable("a", "flag", Value::Bool(true)).unwrap();
    tick(&mut rt, &mut world, 1);
    assert_eq!(state(&rt), 2);
}

#[test]
fn a_bad_predicate_drops_the_call_with_an_error_instead_of_retrying() {
    let (mut rt, mut world) = start(class(
        vec![import("wait::until", vec![Type::Str], Type::Unit)],
        vec![Constant::Str("nothing_by_this_name".into())],
        vec![Const { dst: 1, index: 2 }, CallNative { import: 0, args: vec![1], dst: None }],
        vec![Type::Str],
    ));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    let errors = rt.tick_all(&mut world, TICK);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(rt.waiting_calls("a"), 0, "not retried every tick");
    assert!(rt.tick_all(&mut world, TICK).is_empty());
}

#[test]
fn wait_event_resumes_right_after_the_event_is_handled() {
    let (mut rt, mut world) = start(class(
        vec![import("wait::event", vec![Type::Str], Type::Unit)],
        vec![Constant::Str("on_ping".into())],
        vec![Const { dst: 1, index: 2 }, CallNative { import: 0, args: vec![1], dst: None }],
        vec![Type::Str],
    ));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    tick(&mut rt, &mut world, 5);
    assert_eq!(state(&rt), 1, "ticks do not wake an event wait");
    rt.send_event("a", "on_ping", &[], &mut world).unwrap();
    assert_eq!(state(&rt), 2);
    assert_eq!(rt.waiting_calls("a"), 0);
}

/// A class whose `go` calls `native("fire", args..)`.
fn timer_class(native: &str, extra_params: Vec<Type>, args: Vec<Constant>) -> Module {
    let mut params = vec![Type::Str];
    params.extend(extra_params);
    let mut constants = vec![Constant::Str("fire".into())];
    constants.extend(args.clone());
    let mut code = vec![Const { dst: 1, index: 2 }];
    let mut regs = vec![Type::Str];
    let mut call_args = vec![1u16];
    for (i, constant) in args.iter().enumerate() {
        let reg = 2 + i as u16;
        code.push(Const { dst: reg, index: 3 + i as u32 });
        regs.push(constant.ty());
        call_args.push(reg);
    }
    code.push(CallNative { import: 0, args: call_args, dst: None });
    class(vec![import(native, params, Type::Int)], constants, code, regs)
}

#[test]
fn a_timer_fires_once_after_its_delay() {
    let (mut rt, mut world) = start(timer_class("schedule::call", vec![Type::Float], vec![Constant::Float(1.0)]));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    tick(&mut rt, &mut world, 1);
    assert_eq!(fired(&rt), 0);
    tick(&mut rt, &mut world, 1);
    assert_eq!(fired(&rt), 1, "one second elapsed");
    tick(&mut rt, &mut world, 6);
    assert_eq!(fired(&rt), 1, "a one-shot timer does not repeat");
}

#[test]
fn a_repeating_timer_fires_every_interval() {
    let (mut rt, mut world) = start(timer_class("schedule::repeat", vec![Type::Float], vec![Constant::Float(1.0)]));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    tick(&mut rt, &mut world, 6);
    assert_eq!(fired(&rt), 3, "three seconds");
}

#[test]
fn a_long_hitch_does_not_queue_unbounded_timer_calls() {
    let (mut rt, mut world) = start(timer_class("schedule::repeat", vec![Type::Float], vec![Constant::Float(0.01)]));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    let errors = rt.tick_all(&mut world, 60.0);
    assert!(errors.is_empty());
    assert_eq!(fired(&rt), 8, "capped catch-up, the rest skipped");
}

#[test]
fn timers_are_cleared_by_handle() {
    // go(): h = schedule::call("fire", 1.0); schedule::clear(h)
    let mut m = class(
        vec![
            import("schedule::call", vec![Type::Str, Type::Float], Type::Int),
            import("schedule::clear", vec![Type::Int], Type::Bool),
        ],
        vec![Constant::Str("fire".into()), Constant::Float(1.0)],
        vec![
            Const { dst: 1, index: 2 },
            Const { dst: 2, index: 3 },
            CallNative { import: 0, args: vec![1, 2], dst: Some(3) },
            CallNative { import: 1, args: vec![3], dst: Some(4) },
        ],
        vec![Type::Str, Type::Float, Type::Int, Type::Bool],
    );
    m.functions[0].registers = vec![Type::Int, Type::Str, Type::Float, Type::Int, Type::Bool];
    let (mut rt, mut world) = start(m);
    rt.send_event("a", "go", &[], &mut world).unwrap();
    tick(&mut rt, &mut world, 6);
    assert_eq!(fired(&rt), 0);
}

#[test]
fn a_retriggerable_delay_restarts_instead_of_stacking() {
    // go() twice, a second apart: one call, a full delay after the second.
    let mut m = class(
        vec![import("schedule::restart", vec![Type::Str, Type::Str, Type::Float], Type::Int)],
        vec![Constant::Str("debounce".into()), Constant::Str("fire".into()), Constant::Float(1.5)],
        vec![
            Const { dst: 1, index: 2 },
            Const { dst: 2, index: 3 },
            Const { dst: 3, index: 4 },
            CallNative { import: 0, args: vec![1, 2, 3], dst: Some(4) },
        ],
        vec![Type::Str, Type::Str, Type::Float, Type::Int],
    );
    m.functions[0].registers = vec![Type::Int, Type::Str, Type::Str, Type::Float, Type::Int];
    let (mut rt, mut world) = start(m);
    rt.send_event("a", "go", &[], &mut world).unwrap();
    tick(&mut rt, &mut world, 2);
    rt.send_event("a", "go", &[], &mut world).unwrap();
    tick(&mut rt, &mut world, 2);
    assert_eq!(fired(&rt), 0, "the second trigger pushed the call back");
    tick(&mut rt, &mut world, 1);
    assert_eq!(fired(&rt), 1, "1.5 s after the last trigger, once");
    tick(&mut rt, &mut world, 6);
    assert_eq!(fired(&rt), 1);
}

#[test]
fn a_timer_for_a_function_that_stops_existing_is_dropped_on_reload() {
    let (mut rt, mut world) = start(timer_class("schedule::call", vec![Type::Float], vec![Constant::Float(1.0)]));
    rt.send_event("a", "go", &[], &mut world).unwrap();
    let mut without_fire = timer_class("schedule::call", vec![Type::Float], vec![Constant::Float(1.0)]);
    without_fire.functions.retain(|f| f.name != "fire");
    rt.reload_class(without_fire).unwrap();
    assert!(rt.tick_all(&mut world, 2.0).is_empty(), "no error: the timer was dropped at the reload");
}

#[test]
fn latent_natives_fail_clearly_where_there_is_no_latent_state() {
    use std::sync::Arc;
    use pulsar_script_vm::{Budget, Host, NativeRegistry, Program, Vm};
    let module = class(
        vec![import("wait::next_tick", vec![], Type::Unit)],
        vec![],
        vec![CallNative { import: 0, args: vec![], dst: None }],
        vec![],
    );
    let program = Program::link(Arc::new(module), &NativeRegistry::with_engine_natives()).unwrap();
    let mut world = World::new();
    let entity = world.spawn();
    let mut instance = program.instantiate();
    let func = program.entry("go").unwrap();
    let mut host = Host::new(&mut world, entity);
    let error = Vm::new().call(&program, &mut instance, func, &[], &mut host, &mut Budget::new(100)).unwrap_err();
    assert!(error.to_string().contains("no latent actions"), "{error}");
}

#[test]
fn a_call_waiting_on_frames_survives_a_reload_that_keeps_its_code_shape() {
    let build = || {
        class(
            vec![import("wait::frames", vec![Type::Int], Type::Unit)],
            vec![Constant::Int(3)],
            vec![Const { dst: 0, index: 2 }, CallNative { import: 0, args: vec![0], dst: None }],
            vec![],
        )
    };
    let (mut rt, mut world) = start(build());
    rt.send_event("a", "go", &[], &mut world).unwrap();
    tick(&mut rt, &mut world, 1);
    let report = rt.reload_class(build()).unwrap();
    assert!(report.dropped.is_empty(), "{:?}", report.dropped);
    assert_eq!(rt.waiting_calls("a"), 1);
    tick(&mut rt, &mut world, 2);
    assert_eq!(state(&rt), 2, "the countdown carried over");
}
