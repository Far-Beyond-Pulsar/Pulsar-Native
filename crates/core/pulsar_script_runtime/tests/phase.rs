//! The two-stage script phase: read-only classes under a shared world,
//! everything else exclusive.

use pulsar_scenedb::World;
use pulsar_script_runtime::ScriptRuntime;
use pulsar_script_vm::{
    Access, BinOp, Constant, Function, Import, Instr, Module, NativeRegistry, Param, Program, Signature, Type, Value,
    Variable,
};

use Instr::*;

fn runtime() -> ScriptRuntime {
    ScriptRuntime::new(std::env::temp_dir().join(format!("pulsar_script_phase_{}", std::process::id())))
}

/// `tick(delta)`: `count += 1`, then, if `import` is given, calls it with no
/// arguments (e.g. `entity::spawn`).
fn counter(name: &str, import: Option<(&str, Type)>) -> Module {
    let mut m = Module::new(name);
    m.variables = vec![Variable { name: "count".into(), ty: Type::Int, default: None, id: None }];
    m.constants = vec![Constant::Int(1)];
    let mut code = vec![
        LoadVar { dst: 1, var: 0 },
        Const { dst: 2, index: 0 },
        Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
        StoreVar { var: 0, src: 1 },
    ];
    let mut registers = vec![Type::Float, Type::Int, Type::Int];
    if let Some((native, ret)) = import {
        m.imports = vec![Import { name: native.into(), sig: Signature::new(Vec::<Param>::new(), ret.clone()) }];
        registers.push(ret);
        code.push(CallNative { import: 0, args: vec![], dst: Some(3) });
    }
    code.push(Return { value: None });
    m.functions = vec![Function {
        name: "tick".into(),
        exported: true,
        params: vec![Type::Float],
        ret: Type::Unit,
        registers,
        code,
        debug: None,
    }];
    m
}

fn count(rt: &ScriptRuntime, id: &str) -> i64 {
    rt.variable(id, "count").and_then(Value::as_int).unwrap()
}

fn start(readers: usize, writers: usize) -> (ScriptRuntime, World) {
    let mut rt = runtime();
    rt.load_class(counter("Reader", None)).unwrap();
    rt.load_class(counter("Writer", Some(("entity::spawn", Type::Entity)))).unwrap();
    for i in 0..readers {
        rt.spawn(format!("r{i}"), "Reader", None, &[]).unwrap();
    }
    for i in 0..writers {
        rt.spawn(format!("w{i}"), "Writer", None, &[]).unwrap();
    }
    let mut world = World::new();
    assert!(rt.dispatch_pending_begin_play(&mut world).is_empty());
    (rt, world)
}

#[test]
fn a_class_is_read_only_when_every_import_is_read_access() {
    let registry = NativeRegistry::with_engine_natives();
    let link = |m: Module| Program::link(std::sync::Arc::new(m), &registry).unwrap();
    assert_eq!(link(counter("Reader", None)).access(), Access::Read);
    assert_eq!(link(counter("Math", Some(("entity::none", Type::Entity)))).access(), Access::Read, "pure natives read");
    assert_eq!(link(counter("Writer", Some(("entity::spawn", Type::Entity)))).access(), Access::Write);
}

#[test]
fn read_only_instances_run_in_the_read_stage_and_the_rest_in_the_write_stage() {
    let (mut rt, mut world) = start(3, 2);
    let errors = rt.tick_all(&mut world, 0.1);
    assert!(errors.is_empty(), "{errors:?}");
    let stats = rt.phase_stats();
    assert_eq!((stats.read_instances, stats.write_instances), (3, 2));
    for id in ["r0", "r1", "r2", "w0", "w1"] {
        assert_eq!(count(&rt, id), 1, "{id} ticked once");
    }
}

#[test]
fn the_read_stage_needs_only_a_shared_world() {
    let (mut rt, world) = start(4, 2);
    rt.begin_tick(0.1);
    // `&World`: nothing here could write, so nothing is locked exclusively.
    let shared: &World = &world;
    assert!(rt.run_read_stage(shared, 0.1).is_empty());
    assert_eq!(count(&rt, "r0"), 1);
    assert_eq!(count(&rt, "w0"), 0, "writers wait for the write stage");
    let mut world = world;
    assert!(rt.run_write_stage(&mut world, 0.1).is_empty());
    assert_eq!(count(&rt, "w0"), 1);
    assert_eq!(count(&rt, "r0"), 1, "and readers do not run twice");
}

#[test]
fn threads_do_not_change_the_result() {
    let run = |threshold: usize| {
        let (mut rt, mut world) = start(200, 5);
        rt.set_parallel_threshold(threshold);
        for _ in 0..3 {
            assert!(rt.tick_all(&mut world, 0.1).is_empty());
        }
        let threads = rt.phase_stats().read_threads;
        let counts: Vec<i64> = (0..200).map(|i| count(&rt, &format!("r{i}"))).collect();
        (counts, threads)
    };
    let (sequential, one) = run(usize::MAX);
    let (parallel, _) = run(2);
    assert_eq!(one, 1);
    assert_eq!(sequential, parallel);
    assert!(sequential.iter().all(|c| *c == 3));
}

#[test]
fn a_write_native_in_a_read_only_host_fails_the_call() {
    use pulsar_script_vm::{Budget, Host, Vm};
    let registry = NativeRegistry::with_engine_natives();
    let program = Program::link(
        std::sync::Arc::new(counter("Writer", Some(("entity::spawn", Type::Entity)))),
        &registry,
    )
    .unwrap();
    let world = World::new();
    let mut instance = program.instantiate();
    let mut host = Host::read_only(&world, pulsar_scenedb::Entity::DANGLING, 0.0);
    let func = program.module().function("tick").map(|(i, _)| pulsar_script_vm::FuncId(i)).unwrap();
    let error = Vm::new()
        .call(&program, &mut instance, func, &[Value::Float(0.1)], &mut host, &mut Budget::new(1000))
        .unwrap_err();
    assert!(error.to_string().contains("read-only phase"), "{error}");
}
