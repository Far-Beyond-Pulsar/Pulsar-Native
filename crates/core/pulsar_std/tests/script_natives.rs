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
