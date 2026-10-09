//! Script tick macrobenchmark (#853): thousands of script instances each
//! running a `tick` that reads and writes a component through the world's
//! `Transform` natives, against the same work written directly in Rust.
//!
//! `cargo bench -p pulsar_world_registry --bench script_tick`
//! (`-- --quick` for a fast pass.)
//!
//! Three rows per instance count:
//! - `direct`: Rust against the `World`, the floor (what a DirectRust export
//!   does);
//! - `script`: the interpreter running the bytecode `tick`;
//! - `script/frame`: the same, but every instance is a fresh `Vm::call` the
//!   way the runtime drives them (one `Vm` reused).

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pulsar_scene_model::Transform;
use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::{
    Budget, Function, Host, Import, Instr, Module, NativeRegistry, Param, Program, Signature, Type,
    Value, Vm,
};

use pulsar_world_registry as _;

fn transform() -> Type {
    Type::component("Transform")
}

fn vec3() -> Type {
    Type::object("Vec3")
}

/// `tick(me)`: `t = Transform::of(me); p = t.position(); t.set_position(p)`.
/// A component lookup, a read and a write back through the reflected property
/// natives.
fn tick_program() -> Program {
    let mut module = Module::new("bench");
    module.imports = vec![
        Import {
            name: "Transform::of".into(),
            sig: Signature::new(vec![Param::new(Type::Entity)], transform()),
        },
        Import {
            name: "Transform::position".into(),
            sig: Signature::new(vec![Param::new(transform())], vec3()),
        },
        Import {
            name: "Transform::set_position".into(),
            sig: Signature::new(
                vec![Param::new(transform()), Param::new(vec3())],
                Type::Unit,
            ),
        },
    ];
    module.functions = vec![Function {
        name: "tick".into(),
        exported: true,
        params: vec![Type::Entity],
        ret: Type::Unit,
        registers: vec![Type::Entity, transform(), vec3()],
        code: vec![
            Instr::CallNative {
                import: 0,
                args: vec![0],
                dst: Some(1),
            },
            Instr::CallNative {
                import: 1,
                args: vec![1],
                dst: Some(2),
            },
            Instr::CallNative {
                import: 2,
                args: vec![1, 2],
                dst: None,
            },
            Instr::Return { value: None },
        ],
        debug: None,
    }];
    Program::link(Arc::new(module), &NativeRegistry::with_engine_natives()).expect("link")
}

fn world_with(count: usize) -> (World, Vec<Entity>) {
    let mut world = World::new();
    let entities = (0..count)
        .map(|i| {
            let e = world.spawn();
            world.insert(
                e,
                Transform {
                    position: [i as f32, 0.0, 0.0],
                    ..Default::default()
                },
            );
            e
        })
        .collect();
    (world, entities)
}

fn best_of(rounds: u32, mut run: impl FnMut() -> Duration) -> Duration {
    (0..rounds)
        .map(|_| run())
        .min()
        .expect("at least one round")
}

fn main() {
    let quick = std::env::args().any(|a| a == "--quick");
    let rounds = if quick { 2 } else { 7 };
    let program = tick_program();
    let func = program.entry("tick").expect("tick");

    println!(
        "{:>9}  {:<8} {:>12} {:>12}",
        "instances", "row", "per tick", "per instance"
    );
    for count in [1_000usize, 10_000] {
        let (mut world, entities) = world_with(count);

        let direct = best_of(rounds, || {
            let started = Instant::now();
            for &e in &entities {
                let position = world
                    .get::<Transform>(e)
                    .map(|t| t.position)
                    .unwrap_or_default();
                if let Some(mut t) = world.get_mut::<Transform>(e) {
                    t.position = position;
                }
            }
            started.elapsed()
        });

        let mut vm = Vm::new();
        let mut instances: Vec<_> = entities.iter().map(|_| program.instantiate()).collect();
        let scripted = best_of(rounds, || {
            let started = Instant::now();
            for (instance, &e) in instances.iter_mut().zip(&entities) {
                let mut host = Host::new(&mut world, e);
                let result = vm.call(
                    &program,
                    instance,
                    func,
                    &[Value::Entity(e)],
                    &mut host,
                    &mut Budget::new(10_000),
                );
                black_box(result.expect("tick runs"));
            }
            started.elapsed()
        });

        for (row, elapsed) in [("direct", direct), ("script", scripted)] {
            println!(
                "{count:>9}  {row:<8} {:>9.2} ms {:>9.0} ns",
                elapsed.as_secs_f64() * 1e3,
                elapsed.as_secs_f64() * 1e9 / count as f64,
            );
        }
        println!(
            "{:>9}  {:<8} {:>12} {:>9.1}x",
            "",
            "ratio",
            "",
            scripted.as_secs_f64() / direct.as_secs_f64()
        );
    }
}
