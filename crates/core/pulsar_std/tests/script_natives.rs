//! `#[blueprint]` functions with script-representable signatures are
//! registered as script VM natives named `std::<fn>`. (Referencing
//! pulsar_std at all is what links its registrations into a binary.)

use std::sync::Arc;

use pulsar_scenedb::World;
use pulsar_script_vm::{
    Budget, Function, Host, Import, Instr, Module, NativeRegistry, Param, Program,
    Signature, Type, Value, Vm,
};

#[test]
fn blueprint_functions_become_natives() {
    assert!(!pulsar_std::get_all_nodes().is_empty());
    let registry = NativeRegistry::with_engine_natives();
    let std_natives: Vec<_> = registry.functions().filter(|n| n.name.starts_with("std::")).collect();
    eprintln!("{} pulsar_std functions are natives", std_natives.len());
    assert!(std_natives.len() > 100, "only {} std natives", std_natives.len());

    let add = registry.get("std::add").expect("std::add");
    assert_eq!(add.sig, Signature::new([Param::new(Type::Int), Param::new(Type::Int)], Type::Int));
    assert_eq!(add.param_names, ["a", "b"]);
    assert_eq!(add.attr("category"), Some("Math"));
    assert!(add.flags.side_effect_free);

    // `&str` parameters are taken as strings.
    let print = registry.get("std::print_string").expect("std::print_string");
    assert_eq!(print.sig, Signature::new([Param::new(Type::Str)], Type::Unit));
    assert!(!print.flags.side_effect_free);
}

#[test]
fn natives_run_through_the_vm() {
    assert!(!pulsar_std::get_all_nodes().is_empty());
    let registry = NativeRegistry::with_engine_natives();
    let mut module = Module::new("uses_std");
    module.imports = vec![Import {
        name: "std::divide".into(),
        sig: Signature::new([Param::new(Type::Int), Param::new(Type::Int)], Type::Int),
    }];
    module.functions = vec![Function {
        name: "div".into(),
        exported: true,
        params: vec![Type::Int, Type::Int],
        ret: Type::Int,
        registers: vec![Type::Int, Type::Int, Type::Int],
        code: vec![
            Instr::CallNative { import: 0, args: vec![0, 1], dst: Some(2) },
            Instr::Return { value: Some(2) },
        ],
        debug: None,
    }];
    let program = Program::link(Arc::new(module), &registry).unwrap();
    let mut world = World::new();
    let entity = world.spawn();
    let mut vm = Vm::new();
    let mut instance = program.instantiate();
    let func = program.entry("div").unwrap();
    let mut run = |a: i64, b: i64| {
        let mut host = Host::new(&mut world, entity);
        vm.call(&program, &mut instance, func, &[Value::Int(a), Value::Int(b)], &mut host, &mut Budget::new(100))
    };
    assert_eq!(run(12, 4).unwrap(), Value::Int(3));
    // pulsar_std's divide defines x / 0 as 0.
    assert_eq!(run(1, 0).unwrap(), Value::Int(0));
}

#[test]
fn control_flow_nodes_become_selector_natives() {
    assert!(!pulsar_std::get_all_nodes().is_empty());
    let registry = NativeRegistry::with_engine_natives();
    // Fires one of five outputs and returns which (1-5) through `result`.
    let randexec = registry.get("std::randexec").expect("std::randexec");
    assert_eq!(randexec.attr("exec_outputs"), Some("A,B,C,D,E"));
    assert_eq!(randexec.sig, Signature::new([Param::inout(Type::Int)], Type::Int));
    // A body that fires inside a loop must run the graph between firings:
    // no selector (the compiler implements the built-in loops itself).
    assert!(registry.get("std::for_loop").is_none());

    let mut world = World::new();
    let e = world.spawn();
    let mut host = Host::new(&mut world, e);
    for _ in 0..20 {
        let mut args = [Value::Int(0)];
        let fired = randexec.call(&mut host, &mut args).unwrap().as_int().unwrap();
        assert!((0..5).contains(&fired));
        assert_eq!(args[0], Value::Int(fired + 1), "result is the chosen pin number");
    }
    let fired = registry.get("std::branch").unwrap().call(&mut host, &mut [Value::Bool(false)]).unwrap();
    assert_eq!(fired, Value::Int(1), "branch(false) fires False");
}

/// #869: natives of file, process, network and environment nodes are
/// capability-gated; everything else is not.
#[test]
fn sensitive_natives_carry_a_capability() {
    let registry = NativeRegistry::with_engine_natives();
    assert_eq!(registry.get("std::add").unwrap().capability(), None);
    let mut gated = 0;
    for native in registry.functions().filter(|n| n.name.starts_with("std::")) {
        let expected = match native.attr("category") {
            Some("File I/O") => Some("fs"),
            Some("Process" | "Shell") => Some("process"),
            Some("HTTP" | "Network") => Some("net"),
            Some("Env") => Some("env"),
            _ => None,
        };
        assert_eq!(native.capability(), expected, "{}", native.name);
        gated += usize::from(expected.is_some());
    }
    assert!(gated > 0, "no capability-gated std natives");
}

fn run_import(name: &str, params: Vec<Type>, ret: Type, args: Vec<Value>) -> Result<Value, pulsar_script_vm::ScriptError> {
    let registry = NativeRegistry::with_engine_natives();
    let mut module = Module::new("uses_std");
    module.imports = vec![Import { name: name.into(), sig: Signature::new(params.iter().cloned().map(Param::new), ret.clone()) }];
    let registers: Vec<Type> = params.iter().cloned().chain([ret.clone()]).collect();
    let arg_regs: Vec<u16> = (0..params.len() as u16).collect();
    let result = params.len() as u16;
    module.functions = vec![Function {
        name: "call".into(),
        exported: true,
        params,
        ret,
        registers,
        code: vec![
            Instr::CallNative { import: 0, args: arg_regs, dst: Some(result) },
            Instr::Return { value: Some(result) },
        ],
        debug: None,
    }];
    let program = Program::link(Arc::new(module), &registry).expect("links");
    let mut world = World::new();
    let entity = world.spawn();
    let mut vm = Vm::new();
    let mut instance = program.instantiate();
    let func = program.entry("call").unwrap();
    let mut host = Host::new(&mut world, entity);
    vm.call(&program, &mut instance, func, &args, &mut host, &mut Budget::new(100))
}

#[test]
fn collection_signatures_become_natives() {
    // A `Vec<String>` argument and result, and a `&str`-style borrow.
    let words = Value::list(vec![Value::from("a"), Value::from("b")]);
    let joined = run_import(
        "std::string_join",
        vec![Type::list(Type::Str), Type::Str],
        Type::Str,
        vec![words.clone(), Value::from("-")],
    )
    .unwrap();
    assert_eq!(joined, Value::from("a-b"));
    let split = run_import("std::string_split", vec![Type::Str, Type::Str], Type::list(Type::Str), vec![Value::from("a,b"), Value::from(",")]).unwrap();
    assert_eq!(split, words);

    // A tuple argument and result: vector3 add.
    let v3 = Type::Tuple(vec![Type::Float; 3]);
    let sum = run_import(
        "std::vector3_add",
        vec![v3.clone(), v3.clone()],
        v3,
        vec![
            Value::tuple(vec![Value::Float(1.0), Value::Float(2.0), Value::Float(3.0)]),
            Value::tuple(vec![Value::Float(1.0), Value::Float(1.0), Value::Float(1.0)]),
        ],
    )
    .unwrap();
    assert_eq!(sum, Value::tuple(vec![Value::Float(2.0), Value::Float(3.0), Value::Float(4.0)]));

    // A map in and a list out.
    let registry = NativeRegistry::with_engine_natives();
    assert_eq!(
        registry.get("std::hashmap_keys").expect("native").sig,
        Signature::new([Param::new(Type::map(Type::Str, Type::Str))], Type::list(Type::Str))
    );
}

#[test]
fn fallible_and_optional_results_are_values() {
    let outcome = Type::Tuple(vec![Type::Bool, Type::Int, Type::Str]);
    let parsed = run_import("std::string_to_int", vec![Type::Str], outcome.clone(), vec![Value::from("42")]).unwrap();
    assert_eq!(parsed, Value::tuple(vec![Value::Bool(true), Value::Int(42), Value::from("")]));
    let Value::Tuple(failed) = run_import("std::string_to_int", vec![Type::Str], outcome, vec![Value::from("x")]).unwrap() else {
        panic!("a tuple")
    };
    assert_eq!((&failed[0], &failed[1]), (&Value::Bool(false), &Value::Int(0)));
    assert!(matches!(&failed[2], Value::Str(message) if !message.is_empty()), "the error text is kept: {failed:?}");

    // The pins of a multi-output native are named.
    let registry = NativeRegistry::with_engine_natives();
    assert_eq!(registry.get("std::string_to_int").unwrap().attr("outputs"), Some("ok,value,error"));
}

#[test]
fn generic_array_natives_are_instantiated_per_element_type() {
    let ints = Type::list(Type::Int);
    let pushed = run_import(
        "std::array_push@int",
        vec![ints.clone(), Type::Int],
        ints.clone(),
        vec![Value::list(vec![Value::Int(1)]), Value::Int(2)],
    )
    .unwrap();
    assert_eq!(pushed, Value::list(vec![Value::Int(1), Value::Int(2)]));

    // Another element type, from the same template.
    let strings = Type::list(Type::Str);
    let found = run_import(
        "std::array_contains@string",
        vec![strings.clone(), Type::Str],
        Type::Bool,
        vec![Value::list(vec![Value::from("a")]), Value::from("a")],
    )
    .unwrap();
    assert_eq!(found, Value::Bool(true));

    // An out-of-range `array_get` is absent, with the element's default.
    let option = Type::Tuple(vec![Type::Bool, Type::Int]);
    let missing = run_import("std::array_get@int", vec![ints, Type::Int], option, vec![Value::list(vec![]), Value::Int(3)]).unwrap();
    assert_eq!(missing, Value::tuple(vec![Value::Bool(false), Value::Int(0)]));

    // A signature that is not an instance of the template is refused at link time.
    let registry = NativeRegistry::with_engine_natives();
    let mut module = Module::new("bad");
    module.imports = vec![Import {
        name: "std::array_push@int".into(),
        sig: Signature::new([Param::new(Type::list(Type::Int)), Param::new(Type::Str)], Type::list(Type::Int)),
    }];
    assert!(matches!(Program::link(Arc::new(module), &registry), Err(pulsar_script_vm::LinkError::PolyNative { .. })));
}
