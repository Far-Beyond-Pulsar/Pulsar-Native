//! Instructions, control flow, calls, strings, budgets and errors.

mod common;

use common::{float, int, Asm, Harness};
use pulsar_script_vm::{
    BinOp, Budget, Constant, Host, Instr, Module, NativeRegistry, Param, ScriptErrorKind, Type,
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
