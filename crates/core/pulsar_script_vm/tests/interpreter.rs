//! Instructions, control flow, calls, strings, budgets and errors.

mod common;

use common::{float, int, Asm, Harness};
use pulsar_script_vm::{
    BinOp, Budget, Completion, Constant, Host, Instr, Module, NativeRegistry, Param, ScriptErrorKind, Type,
    UnOp, Value,
};

use Instr::*;

#[test]
fn arithmetic_and_conversions() {
    let mut asm = Asm::new();
    // (a + b) * 2, as float, as string
    let two = asm.constant(Constant::Int(2));
    asm.function(
        "calc",
        vec![Type::Int, Type::Int],
        Type::Str,
        vec![Type::Int, Type::Int, Type::Float, Type::Str],
        vec![
            Binary { op: BinOp::Add, dst: 2, a: 0, b: 1 },
            Const { dst: 3, index: two },
            Binary { op: BinOp::Mul, dst: 2, a: 2, b: 3 },
            Unary { op: UnOp::IntToFloat, dst: 4, src: 2 },
            Unary { op: UnOp::ToStr, dst: 5, src: 4 },
            Return { value: Some(5) },
        ],
    );
    let program = asm.link(&NativeRegistry::new());
    let out = Harness::new().run(&program, "calc", &[int(3), int(4)]).unwrap();
    assert_eq!(out, Value::from("14"));
}

/// sum = 0; i = 1; while i <= n { sum += i; i += 1 }; sum
fn sum_to_n(asm: &mut Asm) -> u32 {
    let zero = asm.constant(Constant::Int(0));
    let one = asm.constant(Constant::Int(1));
    asm.function(
        "sum_to",
        vec![Type::Int],
        Type::Int,
        // r1 sum, r2 i, r3 one, r4 cond
        vec![Type::Int, Type::Int, Type::Int, Type::Bool],
        vec![
            Const { dst: 1, index: zero },
            Const { dst: 2, index: one },
            Const { dst: 3, index: one },
            /* 3 */ Binary { op: BinOp::Le, dst: 4, a: 2, b: 0 },
            Branch { cond: 4, then: 5, otherwise: 8 },
            /* 5 */ Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
            Binary { op: BinOp::Add, dst: 2, a: 2, b: 3 },
            Jump { target: 3 },
            /* 8 */ Return { value: Some(1) },
        ],
    )
}

#[test]
fn loops_run_every_iteration() {
    let mut asm = Asm::new();
    sum_to_n(&mut asm);
    let program = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    assert_eq!(h.run(&program, "sum_to", &[int(100)]).unwrap(), int(5050));
    assert_eq!(h.run(&program, "sum_to", &[int(0)]).unwrap(), int(0));
}

#[test]
fn recursive_calls() {
    let mut asm = Asm::new();
    let two = asm.constant(Constant::Int(2));
    let one = asm.constant(Constant::Int(1));
    // fib(n) = n < 2 ? n : fib(n-1) + fib(n-2)
    asm.function(
        "fib",
        vec![Type::Int],
        Type::Int,
        vec![Type::Int, Type::Bool, Type::Int, Type::Int, Type::Int],
        vec![
            Const { dst: 1, index: two },
            Binary { op: BinOp::Lt, dst: 2, a: 0, b: 1 },
            Branch { cond: 2, then: 3, otherwise: 4 },
            /* 3 */ Return { value: Some(0) },
            /* 4 */ Const { dst: 3, index: one },
            Binary { op: BinOp::Sub, dst: 3, a: 0, b: 3 },
            Call { func: 0, args: vec![3], dst: Some(4) },
            Binary { op: BinOp::Sub, dst: 3, a: 0, b: 1 },
            Call { func: 0, args: vec![3], dst: Some(5) },
            Binary { op: BinOp::Add, dst: 4, a: 4, b: 5 },
            Return { value: Some(4) },
        ],
    );
    let program = asm.link(&NativeRegistry::new());
    assert_eq!(Harness::new().run(&program, "fib", &[int(20)]).unwrap(), int(6765));
}

#[test]
fn deep_recursion_is_an_error_not_a_crash() {
    let mut asm = Asm::new();
    asm.function("forever", vec![], Type::Unit, vec![], vec![
        Call { func: 0, args: vec![], dst: None },
        Return { value: None },
    ]);
    let program = asm.link(&NativeRegistry::new());
    let err = Harness::new().run(&program, "forever", &[]).unwrap_err();
    assert_eq!(err.kind, ScriptErrorKind::StackOverflow);
}

#[test]
fn infinite_loops_exhaust_the_budget() {
    let mut asm = Asm::new();
    asm.function("spin", vec![], Type::Unit, vec![], vec![Jump { target: 0 }]);
    let program = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let mut instance = program.instantiate();
    let mut host = Host::new(&mut h.world, h.entity);
    let mut budget = Budget::new(10_000);
    let err = h
        .vm
        .call(&program, &mut instance, program.entry("spin").unwrap(), &[], &mut host, &mut budget)
        .unwrap_err();
    assert_eq!(err.kind, ScriptErrorKind::BudgetExceeded);
    assert_eq!(budget.remaining, 0);
}

#[test]
fn strings_are_values() {
    let mut asm = Asm::new();
    let registry = NativeRegistry::with_engine_natives();
    let upper = asm.import("string::to_upper", vec![Param::new(Type::Str)], Type::Str);
    let len = asm.import("string::len", vec![Param::new(Type::Str)], Type::Int);
    // Copies of one string, concatenated and transformed repeatedly.
    asm.function(
        "shout",
        vec![Type::Str],
        Type::Str,
        vec![Type::Str, Type::Str, Type::Int],
        vec![
            Move { dst: 1, src: 0 },
            Binary { op: BinOp::Add, dst: 2, a: 0, b: 1 },
            CallNative { import: upper, args: vec![2], dst: Some(2) },
            CallNative { import: len, args: vec![2], dst: Some(3) },
            Return { value: Some(2) },
        ],
    );
    let program = asm.link(&registry);
    let mut h = Harness::new();
    for _ in 0..1000 {
        assert_eq!(h.run(&program, "shout", &[Value::from("ab")]).unwrap(), Value::from("ABAB"));
    }
}

#[test]
fn float_math_via_stdlib() {
    let mut asm = Asm::new();
    let registry = NativeRegistry::with_engine_natives();
    let lerp = asm.import(
        "math::lerp",
        vec![Param::new(Type::Float), Param::new(Type::Float), Param::new(Type::Float)],
        Type::Float,
    );
    asm.function(
        "mid",
        vec![Type::Float, Type::Float],
        Type::Float,
        vec![Type::Float, Type::Float],
        vec![
            Const { dst: 2, index: 0 },
            CallNative { import: lerp, args: vec![0, 1, 2], dst: Some(3) },
            Return { value: Some(3) },
        ],
    );
    asm.module.constants.push(Constant::Float(0.5));
    let program = asm.link(&registry);
    assert_eq!(Harness::new().run(&program, "mid", &[float(2.0), float(4.0)]).unwrap(), float(3.0));
}

#[test]
fn runtime_errors_carry_a_trace() {
    let mut asm = Asm::new();
    asm.function("div", vec![Type::Int, Type::Int], Type::Int, vec![Type::Int], vec![
        Binary { op: BinOp::Div, dst: 2, a: 0, b: 1 },
        Return { value: Some(2) },
    ]);
    asm.function("outer", vec![], Type::Int, vec![Type::Int, Type::Int], vec![
        Call { func: 0, args: vec![0, 1], dst: Some(0) },
        Return { value: Some(0) },
    ]);
    let program = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let err = h.run(&program, "outer", &[]).unwrap_err();
    assert_eq!(err.kind, ScriptErrorKind::DivideByZero);
    assert_eq!(err.trace, vec![("div".to_string(), 0), ("outer".to_string(), 0)]);
    // The VM is reusable after an error.
    assert_eq!(h.run(&program, "div", &[int(9), int(3)]).unwrap(), int(3));
}

#[test]
fn native_errors_name_the_native() {
    let mut asm = Asm::new();
    let parse = asm.import("string::parse_int", vec![Param::new(Type::Str)], Type::Int);
    asm.function("parse", vec![Type::Str], Type::Int, vec![Type::Int], vec![
        CallNative { import: parse, args: vec![0], dst: Some(1) },
        Return { value: Some(1) },
    ]);
    let program = asm.link(&NativeRegistry::with_engine_natives());
    let mut h = Harness::new();
    assert_eq!(h.run(&program, "parse", &[Value::from(" 42 ")]).unwrap(), int(42));
    let err = h.run(&program, "parse", &[Value::from("nope")]).unwrap_err();
    assert!(matches!(&err.kind, ScriptErrorKind::Native { name, .. } if name == "string::parse_int"));
}

#[test]
fn instance_variables_persist_across_calls() {
    let mut asm = Asm::new();
    let count = asm.var("count", Type::Int, Some(Constant::Int(10)));
    let one = asm.constant(Constant::Int(1));
    asm.function("tick", vec![], Type::Int, vec![Type::Int, Type::Int], vec![
        LoadVar { dst: 0, var: count },
        Const { dst: 1, index: one },
        Binary { op: BinOp::Add, dst: 0, a: 0, b: 1 },
        StoreVar { var: count, src: 0 },
        Return { value: Some(0) },
    ]);
    let program = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let mut a = program.instantiate();
    let mut b = program.instantiate();
    h.call(&program, &mut a, "tick", &[]).unwrap();
    assert_eq!(h.call(&program, &mut a, "tick", &[]).unwrap(), int(12));
    assert_eq!(h.call(&program, &mut b, "tick", &[]).unwrap(), int(11));

    let index = program.variable("count").unwrap();
    program.set_var(&mut a, index, int(100)).unwrap();
    assert_eq!(h.call(&program, &mut a, "tick", &[]).unwrap(), int(101));
    assert!(program.set_var(&mut a, index, float(1.0)).is_err());
}

#[test]
fn self_entity_is_the_bound_entity() {
    let mut asm = Asm::new();
    asm.function("me", vec![], Type::Entity, vec![Type::Entity], vec![
        SelfEntity { dst: 0 },
        Return { value: Some(0) },
    ]);
    let program = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let entity = h.entity;
    assert_eq!(h.run(&program, "me", &[]).unwrap(), Value::Entity(entity));
}

#[test]
fn entry_calls_are_checked() {
    let mut asm = Asm::new();
    sum_to_n(&mut asm);
    let program = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let err = h.run(&program, "sum_to", &[float(1.0)]).unwrap_err();
    assert!(matches!(err.kind, ScriptErrorKind::BadEntryCall(_)));
    let err = h.run(&program, "sum_to", &[]).unwrap_err();
    assert!(matches!(err.kind, ScriptErrorKind::BadEntryCall(_)));
}

#[test]
fn modules_round_trip_through_json() {
    let mut asm = Asm::new();
    sum_to_n(&mut asm);
    asm.import("Health::damage", vec![Param::new(Type::component("Health")), Param::inout(Type::Float)], Type::Unit);
    let json = asm.module.to_json().unwrap();
    assert_eq!(Module::from_json(&json).unwrap(), asm.module);
}

#[test]
fn instances_belong_to_their_module() {
    let mut asm = Asm::new();
    asm.var("n", Type::Int, None);
    asm.function("f", vec![], Type::Unit, vec![], vec![Return { value: None }]);
    let a = asm.link(&NativeRegistry::new());
    let b = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let mut instance = a.instantiate();
    let err = h.call(&b, &mut instance, "f", &[]).unwrap_err();
    assert!(matches!(err.kind, ScriptErrorKind::BadEntryCall(_)));
    h.call(&a, &mut instance, "f", &[]).unwrap();
}

#[test]
fn panicking_natives_fail_the_call() {
    let mut registry = NativeRegistry::new();
    registry
        .register(pulsar_script_vm::NativeFn::builder("test::boom").build(|| -> i64 { panic!("kaboom") }))
        .unwrap();
    let mut asm = Asm::new();
    let boom = asm.import("test::boom", vec![], Type::Int);
    asm.function("f", vec![], Type::Int, vec![Type::Int], vec![
        CallNative { import: boom, args: vec![], dst: Some(0) },
        Return { value: Some(0) },
    ]);
    let program = asm.link(&registry);
    let mut h = Harness::new();
    let err = h.run(&program, "f", &[]).unwrap_err();
    assert!(
        matches!(&err.kind, ScriptErrorKind::Native { name, message } if name == "test::boom" && message == "panicked: kaboom"),
        "{err}"
    );
    // Still usable.
    assert!(h.run(&program, "f", &[]).is_err());
}

#[test]
fn waits_suspend_and_resume_with_all_state() {
    use pulsar_script_vm::Completion;
    let mut asm = Asm::new();
    let log = asm.var("log", Type::Str, None);
    let one = asm.constant(Constant::Float(1.5));
    let a = asm.constant(Constant::Str("a".into()));
    let b = asm.constant(Constant::Str("b".into()));
    // inner(): log += "a"; wait 1.5; log += "b"; return now
    let inner = asm.function("inner", vec![], Type::Float, vec![Type::Str, Type::Str, Type::Float], vec![
        LoadVar { dst: 0, var: log },
        Const { dst: 1, index: a },
        Binary { op: BinOp::Add, dst: 0, a: 0, b: 1 },
        StoreVar { var: log, src: 0 },
        Const { dst: 2, index: one },
        Wait { seconds: 2 },
        LoadVar { dst: 0, var: log },
        Const { dst: 1, index: b },
        Binary { op: BinOp::Add, dst: 0, a: 0, b: 1 },
        StoreVar { var: log, src: 0 },
        Now { dst: 2 },
        Return { value: Some(2) },
    ]);
    // outer(x): t = inner(); return t + x   (x survives the wait)
    asm.function("outer", vec![Type::Float], Type::Float, vec![Type::Float], vec![
        Call { func: inner, args: vec![], dst: Some(1) },
        Binary { op: BinOp::Add, dst: 1, a: 1, b: 0 },
        Return { value: Some(1) },
    ]);
    let program = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let mut instance = program.instantiate();
    let outer = program.entry("outer").unwrap();

    let mut host = Host::at_time(&mut h.world, h.entity, 10.0);
    let first = h.vm.start(&program, &mut instance, outer, &[float(100.0)], &mut host, &mut Budget::new(100)).unwrap();
    let Completion::Waiting { seconds, continuation } = first else { panic!("expected a wait") };
    assert_eq!(seconds, 1.5);
    let log_index = program.variable("log").unwrap();
    assert_eq!(program.var(&instance, log_index), Some(&Value::from("a")));

    // The VM is free for other calls while this one waits.
    assert_eq!(h.vm.call(&program, &mut program.instantiate(), program.entry("inner").unwrap(), &[], &mut Host::new(&mut h.world, h.entity), &mut Budget::new(100)).unwrap_err().kind, ScriptErrorKind::Suspended);

    let mut host = Host::at_time(&mut h.world, h.entity, 11.5);
    let done = h.vm.resume(&program, &mut instance, continuation, &mut host, &mut Budget::new(100)).unwrap();
    let Completion::Returned(value) = done else { panic!("expected a return") };
    assert_eq!(value, float(111.5));
    assert_eq!(program.var(&instance, log_index), Some(&Value::from("ab")));
}

#[test]
fn continuations_only_resume_in_their_program() {
    use pulsar_script_vm::Completion;
    let mut asm = Asm::new();
    let zero = asm.constant(Constant::Float(0.0));
    asm.function("f", vec![], Type::Unit, vec![Type::Float], vec![
        Const { dst: 0, index: zero },
        Wait { seconds: 0 },
        Return { value: None },
    ]);
    let a = asm.link(&NativeRegistry::new());
    let b = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let mut ia = a.instantiate();
    let Completion::Waiting { continuation, .. } = h
        .vm
        .start(&a, &mut ia, a.entry("f").unwrap(), &[], &mut Host::new(&mut h.world, h.entity), &mut Budget::new(10))
        .unwrap()
    else {
        panic!()
    };
    let mut ib = b.instantiate();
    let err = h.vm.resume(&b, &mut ib, continuation, &mut Host::new(&mut h.world, h.entity), &mut Budget::new(10)).unwrap_err();
    assert!(matches!(err.kind, ScriptErrorKind::BadEntryCall(_)));
}

/// #854: debug info maps each frame of a trace to its source location, and
/// the verifier rejects a table that does not fit the code.
#[test]
fn traces_resolve_source_locations() {
    use pulsar_script_vm::{DebugInfo, DebugRange, SourceLoc};
    let mut asm = Asm::new();
    asm.function("div", vec![Type::Int, Type::Int], Type::Int, vec![Type::Int], vec![
        Binary { op: BinOp::Div, dst: 2, a: 0, b: 1 },
        Return { value: Some(2) },
    ]);
    asm.function("outer", vec![], Type::Int, vec![Type::Int, Type::Int], vec![
        Call { func: 0, args: vec![0, 1], dst: Some(0) },
        Return { value: Some(0) },
    ]);
    let mut debug = DebugInfo::default();
    debug.record(0, &SourceLoc::node("g.json", "div_node"));
    asm.module.functions[0].debug = Some(debug);
    let mut debug = DebugInfo::default();
    debug.record(0, &SourceLoc::node("g.json", "call_node"));
    debug.record(1, &SourceLoc::node("g.json", "call_node"));
    assert_eq!(debug.ranges.len(), 1, "contiguous pcs of one node merge");
    asm.module.functions[1].debug = Some(debug);
    let program = asm.link(&NativeRegistry::new());
    let err = Harness::new().run(&program, "outer", &[]).unwrap_err();
    assert_eq!(err.locations.len(), 2);
    assert_eq!(err.location().map(|l| l.node.as_str()), Some("div_node"));
    assert_eq!(err.locations[1].as_ref().map(|l| l.node.as_str()), Some("call_node"));
    assert!(err.to_string().contains("div@0 (node div_node in g.json)"), "{err}");

    // The table survives JSON, and a range past the code fails verification.
    let json = asm.module.to_json().unwrap();
    assert_eq!(Module::from_json(&json).unwrap(), asm.module);
    let mut bad = asm.module.clone();
    bad.functions[0].debug = Some(DebugInfo {
        ranges: vec![DebugRange { start: 1, end: 5, loc: SourceLoc::default() }],
    });
    assert!(pulsar_script_vm::verify(&bad).is_err());
}

/// #862: a continuation moves onto a reloaded module whose waiting
/// functions keep their layout, through nested calls, and is refused when
/// a waiting function changed shape.
#[test]
fn continuations_rebase_onto_compatible_modules() {
    let mut asm = Asm::new();
    let one = asm.constant(Constant::Float(1.0));
    let seven = asm.constant(Constant::Int(7));
    asm.function("inner", vec![], Type::Int, vec![Type::Float, Type::Int], vec![
        Const { dst: 0, index: one },
        Wait { seconds: 0 },
        Const { dst: 1, index: seven },
        Return { value: Some(1) },
    ]);
    asm.function("outer", vec![], Type::Int, vec![Type::Int], vec![
        Call { func: 0, args: vec![], dst: Some(0) },
        Return { value: Some(0) },
    ]);
    let program = asm.link(&NativeRegistry::new());
    let mut h = Harness::new();
    let mut instance = program.instantiate();
    let func = program.entry("outer").unwrap();
    let mut host = Host::new(&mut h.world, h.entity);
    let Completion::Waiting { continuation, .. } =
        h.vm.start(&program, &mut instance, func, &[], &mut host, &mut Budget::new(1000)).unwrap()
    else {
        panic!("waits");
    };
    assert_eq!(continuation.functions(), ["outer", "inner"]);

    // v2: a different constant, same layout, functions reordered.
    let mut v2 = asm.module.clone();
    v2.constants[seven as usize] = Constant::Int(8);
    v2.functions.swap(0, 1);
    if let Call { func, .. } = &mut v2.functions[0].code[0] {
        *func = 1;
    }
    let v2 = std::sync::Arc::new(v2);
    let rebased = continuation.rebase(&v2).expect("compatible");
    let program2 = pulsar_script_vm::Program::link(v2, &NativeRegistry::new()).unwrap();
    let mut instance2 = program2.instantiate();
    let mut host = Host::new(&mut h.world, h.entity);
    match h.vm.resume(&program2, &mut instance2, rebased, &mut host, &mut Budget::new(1000)).unwrap() {
        Completion::Returned(value) => assert_eq!(value, int(8), "finished in the new code"),
        Completion::Waiting { .. } => panic!("finished"),
    }

    // v3: `inner` gained an instruction: refused.
    let mut v3 = asm.module.clone();
    v3.functions[0].code.insert(2, Move { dst: 1, src: 1 });
    let error = continuation.rebase(&std::sync::Arc::new(v3)).unwrap_err();
    assert!(error.contains("inner") && error.contains("instruction count"), "{error}");
    // v4: `inner` no longer waits there.
    let mut v4 = asm.module.clone();
    v4.functions[0].code[1] = Move { dst: 1, src: 1 };
    assert!(continuation.rebase(&std::sync::Arc::new(v4)).unwrap_err().contains("no longer waits"));
}
