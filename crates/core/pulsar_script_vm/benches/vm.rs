//! Interpreter microbenchmarks (#853): what one instruction, one call, one
//! native call and one variable access cost, so optimisation is measured
//! rather than guessed.
//!
//! `cargo bench -p pulsar_script_vm --bench vm`
//! (`-- --quick` for a fast pass; a name fragment selects cases.)
//!
//! A custom harness: no dependency, and the output is the table the headless
//! benchmark list quotes (ns per loop iteration, and per executed instruction).

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pulsar_scenedb::World;
use pulsar_script_vm::{
    BinOp, Budget, CollOp, Constant, Function, Host, Import, Instr, Module, NativeRegistry, Param,
    Program, Signature, Type, Value, Variable, Vm,
};

use Instr::*;

const ITERATIONS: i64 = 100_000;

fn program(module: Module) -> Program {
    Program::link(Arc::new(module), &NativeRegistry::with_engine_natives()).expect("link")
}

fn function(
    name: &str,
    params: Vec<Type>,
    ret: Type,
    registers: Vec<Type>,
    code: Vec<Instr>,
) -> Function {
    Function {
        name: name.into(),
        exported: true,
        params,
        ret,
        registers,
        code,
        debug: None,
    }
}

/// `run(n)`: `for i in 0..n { <body> }`. Registers: 0 = n, 1 = i, 2 = one,
/// 3 = cond, then `extra`. The loop itself is four instructions per
/// iteration (test, branch, increment, jump) plus the body.
fn looped(
    name: &str,
    extra: Vec<Type>,
    body: Vec<Instr>,
    tweak: impl FnOnce(&mut Module),
) -> Module {
    let mut m = Module::new(name);
    m.constants = vec![Constant::Int(1)];
    let mut registers = vec![Type::Int, Type::Int, Type::Int, Type::Bool];
    registers.extend(extra);
    let body_start = 3u32;
    let incr = body_start + body.len() as u32;
    let mut code = vec![
        /* 0 */ Const { dst: 2, index: 0 },
        /* 1 */
        Binary {
            op: BinOp::Lt,
            dst: 3,
            a: 1,
            b: 0,
        },
        /* 2 */
        Branch {
            cond: 3,
            then: body_start,
            otherwise: incr + 2,
        },
    ];
    code.extend(body);
    code.push(Binary {
        op: BinOp::Add,
        dst: 1,
        a: 1,
        b: 2,
    });
    code.push(Jump { target: 1 });
    code.push(Return { value: None });
    m.functions = vec![function(
        "run",
        vec![Type::Int],
        Type::Unit,
        registers,
        code,
    )];
    tweak(&mut m);
    m
}

struct Case {
    name: &'static str,
    program: Program,
    /// Instructions executed per iteration, for the per-instruction column.
    per_iteration: u64,
    argument: i64,
}

/// `run(n)` calls `deep(0)` n times; `deep(d)` recurses to depth 200.
fn call_depth() -> Module {
    let mut m = Module::new("depth");
    m.constants = vec![Constant::Int(1), Constant::Int(200), Constant::Int(0)];
    m.functions = vec![
        function(
            "run",
            vec![Type::Int],
            Type::Unit,
            vec![
                Type::Int,
                Type::Int,
                Type::Int,
                Type::Bool,
                Type::Int,
                Type::Int,
            ],
            vec![
                /* 0 */ Const { dst: 2, index: 0 },
                /* 1 */
                Binary {
                    op: BinOp::Lt,
                    dst: 3,
                    a: 1,
                    b: 0,
                },
                /* 2 */
                Branch {
                    cond: 3,
                    then: 3,
                    otherwise: 7,
                },
                /* 3 */ Const { dst: 4, index: 2 },
                /* 4 */
                Call {
                    func: 1,
                    args: vec![4],
                    dst: Some(5),
                },
                /* 5 */
                Binary {
                    op: BinOp::Add,
                    dst: 1,
                    a: 1,
                    b: 2,
                },
                /* 6 */ Jump { target: 1 },
                /* 7 */ Return { value: None },
            ],
        ),
        function(
            "deep",
            vec![Type::Int],
            Type::Int,
            vec![Type::Int, Type::Int, Type::Int, Type::Bool, Type::Int],
            vec![
                /* 0 */ Const { dst: 1, index: 0 },
                /* 1 */ Const { dst: 4, index: 1 },
                /* 2 */
                Binary {
                    op: BinOp::Add,
                    dst: 2,
                    a: 0,
                    b: 1,
                },
                /* 3 */
                Binary {
                    op: BinOp::Lt,
                    dst: 3,
                    a: 0,
                    b: 4,
                },
                /* 4 */
                Branch {
                    cond: 3,
                    then: 5,
                    otherwise: 7,
                },
                /* 5 */
                Call {
                    func: 1,
                    args: vec![2],
                    dst: Some(4),
                },
                /* 6 */ Return { value: Some(4) },
                /* 7 */ Return { value: Some(0) },
            ],
        ),
    ];
    m.functions[1].exported = false;
    m
}

fn cases() -> Vec<Case> {
    vec![
        // The floor every other case is read against.
        Case {
            name: "empty loop",
            program: program(looped("empty", vec![], vec![], |_| {})),
            per_iteration: 4,
            argument: ITERATIONS,
        },
        Case {
            name: "arithmetic (3 adds)",
            program: program(looped(
                "arith",
                vec![Type::Int],
                vec![
                    Binary {
                        op: BinOp::Add,
                        dst: 4,
                        a: 4,
                        b: 2,
                    },
                    Binary {
                        op: BinOp::Add,
                        dst: 4,
                        a: 4,
                        b: 2,
                    },
                    Binary {
                        op: BinOp::Add,
                        dst: 4,
                        a: 4,
                        b: 2,
                    },
                ],
                |_| {},
            )),
            per_iteration: 7,
            argument: ITERATIONS,
        },
        Case {
            name: "variable load/store",
            program: program(looped(
                "vars",
                vec![Type::Int],
                vec![
                    LoadVar { dst: 4, var: 0 },
                    Binary {
                        op: BinOp::Add,
                        dst: 4,
                        a: 4,
                        b: 2,
                    },
                    StoreVar { var: 0, src: 4 },
                ],
                |m| {
                    m.variables = vec![Variable {
                        name: "count".into(),
                        ty: Type::Int,
                        default: None,
                        id: None,
                    }]
                },
            )),
            per_iteration: 7,
            argument: ITERATIONS,
        },
        // A call to a leaf that adds one to its argument: frame push, register
        // setup, return.
        Case {
            name: "call (leaf, 1 arg)",
            program: program(looped(
                "call",
                vec![Type::Int],
                vec![Call {
                    func: 1,
                    args: vec![1],
                    dst: Some(4),
                }],
                |m| {
                    let mut leaf = function(
                        "leaf",
                        vec![Type::Int],
                        Type::Int,
                        vec![Type::Int, Type::Int],
                        vec![
                            Const { dst: 1, index: 0 },
                            Binary {
                                op: BinOp::Add,
                                dst: 0,
                                a: 0,
                                b: 1,
                            },
                            Return { value: Some(0) },
                        ],
                    );
                    leaf.exported = false;
                    m.functions.push(leaf);
                },
            )),
            per_iteration: 4 + 1 + 3,
            argument: ITERATIONS,
        },
        // 200 nested frames, 500 times; each frame is 7 instructions.
        Case {
            name: "call depth 200",
            program: program(call_depth()),
            per_iteration: 4 + 201 * 7,
            argument: 500,
        },
        // The cost of marshalling and the `catch_unwind` around every native.
        Case {
            name: "native call (no args)",
            program: program(looped(
                "native",
                vec![Type::Entity],
                vec![CallNative {
                    import: 0,
                    args: vec![],
                    dst: Some(4),
                }],
                |m| {
                    m.imports = vec![Import {
                        name: "entity::none".into(),
                        sig: Signature::new(Vec::<Param>::new(), Type::Entity),
                    }];
                },
            )),
            per_iteration: 5,
            argument: ITERATIONS,
        },
        // Copy-on-write collection edited in place.
        Case {
            name: "list push",
            program: program(looped(
                "list",
                vec![Type::list(Type::Int)],
                vec![Collection {
                    op: CollOp::ListPush,
                    dst: 4,
                    args: vec![4, 1],
                }],
                |_| {},
            )),
            per_iteration: 5,
            argument: ITERATIONS,
        },
    ]
}

fn measure(case: &Case, rounds: u32) -> Duration {
    let mut world = World::new();
    let me = world.spawn();
    let func = case.program.entry("run").expect("run");
    let mut vm = Vm::new();
    let mut best = Duration::MAX;
    for _ in 0..rounds {
        let mut instance = case.program.instantiate();
        let mut host = Host::new(&mut world, me);
        let mut budget = Budget::new(u64::MAX / 2);
        let started = Instant::now();
        let result = vm.call(
            &case.program,
            &mut instance,
            func,
            &[Value::Int(case.argument)],
            &mut host,
            &mut budget,
        );
        let elapsed = started.elapsed();
        black_box(result.expect("benchmark program runs"));
        best = best.min(elapsed);
    }
    best
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let quick = args.iter().any(|a| a == "--quick");
    let filter = args.iter().find(|a| !a.starts_with('-')).cloned();
    let rounds = if quick { 3 } else { 15 };

    println!("{:<24} {:>12} {:>13}", "case", "per iter", "per instr");
    for case in cases() {
        if filter.as_deref().is_some_and(|f| !case.name.contains(f)) {
            continue;
        }
        let best = measure(&case, rounds);
        let per_iter = best.as_secs_f64() * 1e9 / case.argument as f64;
        let per_instr = per_iter / case.per_iteration as f64;
        println!(
            "{:<24} {:>9.1} ns {:>10.2} ns",
            case.name, per_iter, per_instr
        );
    }
}
