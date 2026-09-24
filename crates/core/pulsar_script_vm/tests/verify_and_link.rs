//! Modules that must be rejected before they run.

mod common;

use std::sync::Arc;

use common::Asm;
use pulsar_script_vm::{
    BinOp, Constant, Instr, LinkError, NativeFn, NativeRegistry, Param, Program, Type, UnOp,
};

use Instr::*;

fn rejected(asm: &Asm) -> String {
    match Program::link(Arc::new(asm.module.clone()), &NativeRegistry::with_engine_natives()) {
        Ok(_) => panic!("module was accepted"),
        Err(err) => err.to_string(),
    }
}

fn one_function(params: Vec<Type>, ret: Type, regs: Vec<Type>, code: Vec<Instr>) -> Asm {
    let mut asm = Asm::new();
    asm.function("f", params, ret, regs, code);
    asm
}

#[test]
fn operand_type_mismatches() {
    let asm = one_function(vec![Type::Int, Type::Float], Type::Int, vec![Type::Int], vec![
        Binary { op: BinOp::Add, dst: 2, a: 0, b: 1 },
        Return { value: Some(2) },
    ]);
    assert!(rejected(&asm).contains("r1 is float, expected int"), "{}", rejected(&asm));

    let asm = one_function(vec![Type::Str], Type::Str, vec![], vec![
        Unary { op: UnOp::Neg, dst: 0, src: 0 },
        Return { value: Some(0) },
    ]);
    assert!(rejected(&asm).contains("Neg does not apply to string"));

    let asm = one_function(vec![Type::Int], Type::Unit, vec![], vec![
        Branch { cond: 0, then: 1, otherwise: 1 },
        Return { value: None },
    ]);
    assert!(rejected(&asm).contains("expected bool"));
}

#[test]
fn indices_out_of_range() {
    let asm = one_function(vec![], Type::Unit, vec![], vec![Move { dst: 0, src: 1 }, Return { value: None }]);
    assert!(rejected(&asm).contains("out of range"));

    let asm = one_function(vec![], Type::Unit, vec![], vec![Jump { target: 7 }]);
    assert!(rejected(&asm).contains("jump target 7 out of range"));

    let asm = one_function(vec![], Type::Unit, vec![Type::Int], vec![
        Const { dst: 0, index: 3 },
        Return { value: None },
    ]);
    assert!(rejected(&asm).contains("constant 3 out of range"));

    let asm = one_function(vec![], Type::Unit, vec![], vec![
        CallNative { import: 0, args: vec![], dst: None },
        Return { value: None },
    ]);
    assert!(rejected(&asm).contains("import 0 out of range"));
}

#[test]
fn control_cannot_fall_off_the_end() {
    let asm = one_function(vec![], Type::Unit, vec![Type::Int], vec![Const { dst: 0, index: 0 }]);
    assert!(rejected(&asm).contains("must end with"));
}

#[test]
fn returns_match_the_signature() {
    let asm = one_function(vec![], Type::Int, vec![], vec![Return { value: None }]);
    assert!(rejected(&asm).contains("must return a int"));
}

#[test]
fn calls_match_the_callee() {
    let mut asm = Asm::new();
    asm.function("callee", vec![Type::Int], Type::Int, vec![], vec![Return { value: Some(0) }]);
    asm.function("caller", vec![], Type::Unit, vec![Type::Float], vec![
        Call { func: 0, args: vec![0], dst: None },
        Return { value: None },
    ]);
    assert!(rejected(&asm).contains("r0 is float, expected int"));

    let mut asm = Asm::new();
    asm.function("callee", vec![Type::Int], Type::Int, vec![], vec![Return { value: Some(0) }]);
    asm.function("caller", vec![], Type::Unit, vec![], vec![
        Call { func: 0, args: vec![], dst: None },
        Return { value: None },
    ]);
    assert!(rejected(&asm).contains("callee takes 1 arguments, got 0"));
}

#[test]
fn variables_and_defaults_are_typed() {
    let mut asm = Asm::new();
    asm.var("speed", Type::Float, Some(Constant::Int(3)));
    assert!(rejected(&asm).contains("variable `speed` is float but its default is int"));
}

#[test]
fn missing_and_mismatched_natives() {
    let mut asm = Asm::new();
    asm.import("does::not_exist", vec![], Type::Unit);
    let err = Program::link(Arc::new(asm.module), &NativeRegistry::new()).err().unwrap();
    assert_eq!(err, LinkError::MissingNative { name: "does::not_exist".into() });

    let mut asm = Asm::new();
    asm.import("math::sin", vec![Param::new(Type::Int)], Type::Float);
    let err = Program::link(Arc::new(asm.module), &NativeRegistry::with_engine_natives()).err().unwrap();
    assert!(matches!(err, LinkError::SignatureMismatch { .. }), "{err}");
}

#[test]
fn unknown_types_do_not_link() {
    let mut asm = Asm::new();
    asm.function("f", vec![Type::component("NoSuchComponent")], Type::Unit, vec![], vec![Return { value: None }]);
    let err = Program::link(Arc::new(asm.module), &NativeRegistry::new()).err().unwrap();
    assert_eq!(err, LinkError::UnknownType { name: "NoSuchComponent&".into() });
}

#[test]
fn duplicate_natives_are_refused() {
    let mut registry = NativeRegistry::new();
    registry.register(NativeFn::builder("a::b").build(|| 1i64)).unwrap();
    assert!(registry.register(NativeFn::builder("a::b").build(|| 2i64)).is_err());
}

#[test]
fn wrong_format_version() {
    let mut asm = Asm::new();
    asm.module.format_version = 999;
    assert!(rejected(&asm).contains("format version 999"));
}
