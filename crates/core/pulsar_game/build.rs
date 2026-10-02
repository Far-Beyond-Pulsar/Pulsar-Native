//! Generates the Rust-export probe actor.
//!
//! The Blueprint editor's Rust export (`pulsar_script_codegen::actor`) emits
//! an `impl Actor` that every generated game project compiles. The
//! `pulsar_scenedb::Actor` trait it must satisfy is pinned by rev from the
//! root manifest, which the generator cannot see, so drift would otherwise
//! surface only inside user projects (Pulsar-Native#652). This script
//! generates one actor through the real generator and `src/export_probe.rs`
//! includes it into this crate's test build: it is compiled against the real
//! pinned trait on every `cargo test -p pulsar_game`, and any drift is a
//! compile error here.

use std::path::PathBuf;

use pulsar_script_vm::{
    BinOp, Constant, Function, Instr, Module, Type, Variable,
};

fn probe_module() -> Module {
    let mut m = Module::new("export_probe");
    m.variables = vec![
        Variable { name: "beats".into(), ty: Type::Int, default: None, id: None },
        Variable { name: "woke".into(), ty: Type::Int, default: None, id: None },
    ];
    m.constants = vec![Constant::Int(1), Constant::Float(0.0)];
    let func = |name: &str, params: Vec<Type>, extra: Vec<Type>, code: Vec<Instr>| {
        let mut registers = params.clone();
        registers.extend(extra);
        Function { name: name.into(), exported: true, params, ret: Type::Unit, registers, code, debug: None }
    };
    // begin_play: wait (zero seconds: resumes on the next tick), then woke = 1
    m.functions.push(func(
        "begin_play",
        vec![],
        vec![Type::Float, Type::Int],
        vec![
            Instr::Const { dst: 0, index: 1 },
            Instr::Wait { seconds: 0 },
            Instr::Const { dst: 1, index: 0 },
            Instr::StoreVar { var: 1, src: 1 },
            Instr::Return { value: None },
        ],
    ));
    // tick(dt): beats += 1
    m.functions.push(func(
        "tick",
        vec![Type::Float],
        vec![Type::Int, Type::Int],
        vec![
            Instr::LoadVar { dst: 1, var: 0 },
            Instr::Const { dst: 2, index: 0 },
            Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
            Instr::StoreVar { var: 0, src: 1 },
            Instr::Return { value: None },
        ],
    ));
    m
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let source = pulsar_script_codegen::actor::generate_actor("export_probe", &probe_module(), &[])
        .expect("the probe module exports");
    // The generated file is a module file (`events.rs`), whose leading `//!`
    // docs are inner docs. `include!` splices it into an inline module, where
    // they are not allowed: only those comment markers are changed.
    let source: String = source
        .lines()
        .map(|line| match line.strip_prefix("//!") {
            Some(rest) => format!("//{rest}\n"),
            None => format!("{line}\n"),
        })
        .collect();
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("export_probe_actor.rs");
    std::fs::write(out, source).expect("write the probe actor");
}
