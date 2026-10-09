//! Lists, maps and tuples: operations, value semantics, errors, the
//! verifier's type rules and conversion to and from Rust types.

mod common;

use std::collections::{BTreeMap, HashMap};

use common::{int, Asm, Harness};
use pulsar_script_vm::{
    verify, BinOp, CollOp, Constant, Instr, NativeRegistry, ScriptErrorKind, ScriptValue, Type,
    UnOp, Value,
};

use Instr::*;

fn ints() -> Type {
    Type::list(Type::Int)
}

fn coll(op: CollOp, dst: u16, args: &[u16]) -> Instr {
    Collection {
        op,
        dst,
        args: args.to_vec(),
    }
}

#[test]
fn a_list_is_built_read_and_edited() {
    let mut asm = Asm::new();
    let (ten, twenty, zero, one) = (
        asm.constant(Constant::Int(10)),
        asm.constant(Constant::Int(20)),
        asm.constant(Constant::Int(0)),
        asm.constant(Constant::Int(1)),
    );
    // xs = [10, 20]; xs = push(xs, 10); xs = set(xs, 1, xs[0] + 1); xs
    asm.function(
        "build",
        vec![],
        ints(),
        vec![
            Type::Int,
            Type::Int,
            Type::Int,
            Type::Int,
            ints(),
            Type::Int,
        ],
        vec![
            Const { dst: 0, index: ten },
            Const {
                dst: 1,
                index: twenty,
            },
            Const {
                dst: 2,
                index: zero,
            },
            Const { dst: 3, index: one },
            coll(CollOp::MakeList, 4, &[0, 1]),
            coll(CollOp::ListPush, 4, &[4, 0]),
            coll(CollOp::ListGet, 5, &[4, 2]),
            Binary {
                op: BinOp::Add,
                dst: 5,
                a: 5,
                b: 3,
            },
            coll(CollOp::ListSet, 4, &[4, 3, 5]),
            Return { value: Some(4) },
        ],
    );
    let program = asm.link(&NativeRegistry::new());
    let out = Harness::new().run(&program, "build", &[]).unwrap();
    assert_eq!(out, Value::list(vec![int(10), int(11), int(10)]));
}

#[test]
fn collections_have_value_semantics() {
    let mut asm = Asm::new();
    let (nine, zero) = (
        asm.constant(Constant::Int(9)),
        asm.constant(Constant::Int(0)),
    );
    // b = a; b[0] = 9; b
    asm.function(
        "edit_a_copy",
        vec![ints()],
        ints(),
        vec![ints(), Type::Int, Type::Int],
        vec![
            Move { dst: 1, src: 0 },
            Const {
                dst: 2,
                index: nine,
            },
            Const {
                dst: 3,
                index: zero,
            },
            coll(CollOp::ListSet, 1, &[1, 3, 2]),
            Return { value: Some(1) },
        ],
    );
    let program = asm.link(&NativeRegistry::new());
    let original = Value::list(vec![int(1), int(2)]);
    let out = Harness::new()
        .run(&program, "edit_a_copy", std::slice::from_ref(&original))
        .unwrap();
    assert_eq!(out, Value::list(vec![int(9), int(2)]));
    assert_eq!(
        original,
        Value::list(vec![int(1), int(2)]),
        "the caller's list is unchanged"
    );
}

#[test]
fn indexing_out_of_bounds_is_an_error() {
    let mut asm = Asm::new();
    asm.function(
        "at",
        vec![ints(), Type::Int],
        Type::Int,
        vec![Type::Int],
        vec![coll(CollOp::ListGet, 2, &[0, 1]), Return { value: Some(2) }],
    );
    let program = asm.link(&NativeRegistry::new());
    let list = Value::list(vec![int(5), int(6)]);
    let mut harness = Harness::new();
    assert_eq!(
        harness
            .run(&program, "at", &[list.clone(), int(1)])
            .unwrap(),
        int(6)
    );
    for bad in [2, -1] {
        let err = harness
            .run(&program, "at", &[list.clone(), int(bad)])
            .unwrap_err();
        assert_eq!(
            err.kind,
            ScriptErrorKind::IndexOutOfBounds { index: bad, len: 2 }
        );
    }
}

#[test]
fn a_map_is_ordered_and_missing_keys_are_errors() {
    let mut asm = Asm::new();
    let map = Type::map(Type::Str, Type::Int);
    let keys = Type::list(Type::Str);
    asm.function(
        "lookup",
        vec![map.clone(), Type::Str],
        Type::Int,
        vec![Type::Int],
        vec![coll(CollOp::MapGet, 2, &[0, 1]), Return { value: Some(2) }],
    );
    asm.function(
        "keys",
        vec![map.clone()],
        keys.clone(),
        vec![keys],
        vec![coll(CollOp::MapKeys, 1, &[0]), Return { value: Some(1) }],
    );
    let outcome = Type::Tuple(vec![Type::Bool, Type::Int]);
    asm.function(
        "with",
        vec![map.clone(), Type::Str, Type::Int],
        outcome.clone(),
        vec![Type::Bool, Type::Int, outcome],
        vec![
            coll(CollOp::MapSet, 0, &[0, 1, 2]),
            coll(CollOp::MapHas, 3, &[0, 1]),
            coll(CollOp::MapLen, 4, &[0]),
            coll(CollOp::MakeTuple, 5, &[3, 4]),
            Return { value: Some(5) },
        ],
    );
    let program = asm.link(&NativeRegistry::new());
    let mut harness = Harness::new();
    let value = BTreeMap::from([("b".to_owned(), 2i64), ("a".to_owned(), 1)]).into_value();

    assert_eq!(
        harness
            .run(&program, "lookup", &[value.clone(), Value::from("b")])
            .unwrap(),
        int(2)
    );
    let err = harness
        .run(&program, "lookup", &[value.clone(), Value::from("z")])
        .unwrap_err();
    assert_eq!(err.kind, ScriptErrorKind::KeyNotFound { key: "z".into() });
    assert_eq!(
        harness
            .run(&program, "keys", std::slice::from_ref(&value))
            .unwrap(),
        Value::list(vec![Value::from("a"), Value::from("b")]),
        "keys come out in key order"
    );
    assert_eq!(
        harness
            .run(&program, "with", &[value, Value::from("c"), int(3)])
            .unwrap(),
        Value::tuple(vec![Value::Bool(true), int(3)])
    );
}

#[test]
fn tuples_pack_and_unpack() {
    let mut asm = Asm::new();
    let pair = Type::Tuple(vec![Type::Int, Type::Str]);
    asm.function(
        "second",
        vec![Type::Int, Type::Str],
        Type::Str,
        vec![pair, Type::Str],
        vec![
            coll(CollOp::MakeTuple, 2, &[0, 1]),
            coll(CollOp::TupleGet(1), 3, &[2]),
            Return { value: Some(3) },
        ],
    );
    let program = asm.link(&NativeRegistry::new());
    assert_eq!(
        Harness::new()
            .run(&program, "second", &[int(1), Value::from("x")])
            .unwrap(),
        Value::from("x")
    );
}

#[test]
fn the_verifier_checks_collection_types() {
    let bad = |registers: Vec<Type>, code: Vec<Instr>| {
        let mut asm = Asm::new();
        asm.function("f", vec![], Type::Unit, registers, code);
        verify(&asm.module).unwrap_err().to_string()
    };
    // Pushing a string onto a list of ints.
    let message = bad(
        vec![ints(), Type::Str],
        vec![coll(CollOp::ListPush, 0, &[0, 1]), Return { value: None }],
    );
    assert!(message.contains("r1 is string, expected int"), "{message}");
    // A float key.
    let message = bad(
        vec![Type::map(Type::Float, Type::Int)],
        vec![Return { value: None }],
    );
    assert!(
        message.contains("a map key must be bool, int or string"),
        "{message}"
    );
    // A tuple has no element 2.
    let message = bad(
        vec![Type::Tuple(vec![Type::Int, Type::Int]), Type::Int],
        vec![coll(CollOp::TupleGet(2), 1, &[0]), Return { value: None }],
    );
    assert!(message.contains("no element 2"), "{message}");
    // Not a list.
    let message = bad(
        vec![Type::Int, Type::Int],
        vec![coll(CollOp::ListLen, 1, &[0]), Return { value: None }],
    );
    assert!(message.contains("needs a list"), "{message}");
}

#[test]
fn collections_compare_and_print() {
    let mut asm = Asm::new();
    let shown = Type::Tuple(vec![Type::Int, Type::list(Type::Str)]);
    asm.function(
        "same",
        vec![ints(), ints()],
        Type::Bool,
        vec![Type::Bool],
        vec![
            Binary {
                op: BinOp::Eq,
                dst: 2,
                a: 0,
                b: 1,
            },
            Return { value: Some(2) },
        ],
    );
    asm.function(
        "show",
        vec![shown.clone()],
        Type::Str,
        vec![Type::Str],
        vec![
            Unary {
                op: UnOp::ToStr,
                dst: 1,
                src: 0,
            },
            Return { value: Some(1) },
        ],
    );
    let program = asm.link(&NativeRegistry::new());
    let mut harness = Harness::new();
    let a = Value::list(vec![int(1), int(2)]);
    assert_eq!(
        harness
            .run(&program, "same", &[a.clone(), a.clone()])
            .unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        harness
            .run(&program, "same", &[a, Value::list(vec![int(1)])])
            .unwrap(),
        Value::Bool(false)
    );
    let tuple = Value::tuple(vec![
        int(1),
        Value::list(vec![Value::from("a"), Value::from("b")]),
    ]);
    assert_eq!(
        harness.run(&program, "show", &[tuple]).unwrap(),
        Value::from("(1, [a, b])")
    );
}

#[test]
fn rust_collections_convert_both_ways() {
    let v = vec![1i32, 2, 3];
    assert_eq!(<Vec<i32>>::script_type(), Type::list(Type::Int));
    assert_eq!(<Vec<i32>>::from_value(&v.clone().into_value()), Some(v));

    // Arrays check their length.
    let array = [1.5f64, 2.5].into_value();
    assert_eq!(<[f64; 2]>::from_value(&array), Some([1.5, 2.5]));
    assert_eq!(<[f64; 3]>::from_value(&array), None);

    let pair = (1i64, "x".to_owned(), true);
    assert_eq!(
        <(i64, String, bool)>::script_type(),
        Type::Tuple(vec![Type::Int, Type::Str, Type::Bool])
    );
    assert_eq!(
        <(i64, String, bool)>::from_value(&pair.clone().into_value()),
        Some(pair)
    );

    let map: HashMap<String, Vec<i64>> = HashMap::from([("a".into(), vec![1, 2])]);
    assert_eq!(
        <HashMap<String, Vec<i64>>>::script_type(),
        Type::map(Type::Str, Type::list(Type::Int))
    );
    assert_eq!(
        <HashMap<String, Vec<i64>>>::from_value(&map.clone().into_value()),
        Some(map)
    );

    // Values beyond a declared type do not convert.
    assert_eq!(
        <Vec<i32>>::from_value(&Value::list(vec![Value::from("x")])),
        None
    );
    assert!(Value::list(vec![int(1)]).fits(&ints()));
    assert!(!Value::list(vec![Value::Bool(true)]).fits(&ints()));
}
