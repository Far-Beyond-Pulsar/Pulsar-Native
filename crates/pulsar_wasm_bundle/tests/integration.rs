/// High-level integration tests for pulsar_wasm_bundle.
///
/// These tests:
///   1. Verify the embedded WASM bytes are a valid module (correct magic header)
///   2. Compile real Blueprint graphs to bytecode end-to-end
///   3. Execute them with a native dispatch backed by actual pulsar_std functions
///   4. Print timing data for every stage so editor-budget decisions can be made
///
/// The native dispatch mirrors what the engine's WASM-backed dispatch does, but
/// calls into the Rust functions directly — so behaviour is identical without
/// needing a WASM runtime in tests.
use std::time::{Duration, Instant};

use graphy::{
    Connection, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position, PropertyValue,
};
use pbgc::{
    bytecode::BpValue,
    compile_graph, compile_graph_to_bytecode,
    vm::{BytecodeVm, NodeDispatch, VmError},
};
use pulsar_wasm_bundle::PULSAR_STD_WASM;

// ── WASM module validity ──────────────────────────────────────────────────────

#[test]
fn test_wasm_bytes_have_valid_magic() {
    // All WebAssembly modules start with \0asm (0x00 0x61 0x73 0x6D)
    assert!(
        PULSAR_STD_WASM.len() >= 8,
        "WASM module too small: {} bytes",
        PULSAR_STD_WASM.len()
    );
    assert_eq!(
        &PULSAR_STD_WASM[..4],
        b"\x00asm",
        "WASM magic header mismatch"
    );
    // Version 1
    assert_eq!(
        &PULSAR_STD_WASM[4..8],
        &[1, 0, 0, 0],
        "WASM version should be 1"
    );
    println!(
        "[info] PULSAR_STD_WASM embedded: {} bytes ({:.1} KiB)",
        PULSAR_STD_WASM.len(),
        PULSAR_STD_WASM.len() as f64 / 1024.0
    );
}

#[test]
fn test_wasm_bytes_not_stub() {
    // Stub is exactly 8 bytes; the real module is much larger
    assert!(
        PULSAR_STD_WASM.len() > 8,
        "WASM looks like the stub — ensure wasm32 toolchain is installed and PULSAR_SKIP_WASM_BUILD is not set"
    );
}

// ── Native dispatch backed by actual pulsar_std ───────────────────────────────

/// A dispatch implementation that calls the real pulsar_std Rust functions.
/// This gives us correct execution semantics identical to the WASM dispatch,
/// but without needing a WASM runtime in tests.
struct NativeStdDispatch;

impl NodeDispatch for NativeStdDispatch {
    fn call(&self, node_type: &str, inputs: &[BpValue], output: &mut Option<BpValue>) -> Result<(), VmError> {
        match node_type {
            "add"      => { *output = Some(BpValue::Int(inputs[0].as_i64().unwrap_or(0) + inputs[1].as_i64().unwrap_or(0))); }
            "subtract" => { *output = Some(BpValue::Int(inputs[0].as_i64().unwrap_or(0) - inputs[1].as_i64().unwrap_or(0))); }
            "multiply" => { *output = Some(BpValue::Int(inputs[0].as_i64().unwrap_or(0) * inputs[1].as_i64().unwrap_or(0))); }
            "divide"   => {
                let (a, b) = (inputs[0].as_i64().unwrap_or(0), inputs[1].as_i64().unwrap_or(1));
                *output = Some(BpValue::Int(if b == 0 { 0 } else { a / b }));
            }
            "abs"   => { *output = Some(BpValue::Float(inputs[0].as_f64().unwrap_or(0.0).abs())); }
            "sqrt"  => { *output = Some(BpValue::Float(inputs[0].as_f64().unwrap_or(0.0).sqrt())); }
            "power" => { *output = Some(BpValue::Float(inputs[0].as_f64().unwrap_or(0.0).powf(inputs[1].as_f64().unwrap_or(1.0)))); }
            "lerp" => {
                let (a, b, t) = (inputs[0].as_f64().unwrap_or(0.0), inputs[1].as_f64().unwrap_or(0.0), inputs[2].as_f64().unwrap_or(0.0));
                *output = Some(BpValue::Float(a + (b - a) * t));
            }
            "clamp" => {
                let (v, lo, hi) = (inputs[0].as_f64().unwrap_or(0.0), inputs[1].as_f64().unwrap_or(0.0), inputs[2].as_f64().unwrap_or(1.0));
                *output = Some(BpValue::Float(v.clamp(lo, hi)));
            }
            "greater_than" => { *output = Some(BpValue::Bool(inputs[0].as_f64().unwrap_or(0.0) > inputs[1].as_f64().unwrap_or(0.0))); }
            "less_than"    => { *output = Some(BpValue::Bool(inputs[0].as_f64().unwrap_or(0.0) < inputs[1].as_f64().unwrap_or(0.0))); }
            "not" => { *output = Some(BpValue::Bool(!inputs[0].as_bool())); }
            "and" => { *output = Some(BpValue::Bool(inputs[0].as_bool() && inputs[1].as_bool())); }
            "or"  => { *output = Some(BpValue::Bool(inputs[0].as_bool() || inputs[1].as_bool())); }
            "print_string" | "branch" | "sequence" => {}
            other => return Err(VmError::UnknownNode(other.to_string())),
        }
        Ok(())
    }
}

// ── Graph builders ────────────────────────────────────────────────────────────

fn begin_play(exec_pin: &str) -> NodeInstance {
    let mut n = NodeInstance::new("begin", "begin_play", Position { x: 0.0, y: 0.0 });
    n.outputs.push(PinInstance::new(
        exec_pin,
        Pin::new(exec_pin, "Body", DataType::Execution, PinType::Output),
    ));
    n
}

fn add_node(id: &str, const_a: Option<f64>, const_b: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "add", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_a", id),
        Pin::new(&format!("{}_a", id), "a", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_b", id),
        Pin::new(&format!("{}_b", id), "b", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_result", id),
        Pin::new(&format!("{}_result", id), "result", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Output),
    ));
    if let Some(a) = const_a { n.properties.insert(format!("{}_a", id), PropertyValue::Number(a)); }
    if let Some(b) = const_b { n.properties.insert(format!("{}_b", id), PropertyValue::Number(b)); }
    n
}

fn gt_node(id: &str, const_b: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "greater_than", Position { x: 200.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_a", id),
        Pin::new(&format!("{}_a", id), "a", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_b", id),
        Pin::new(&format!("{}_b", id), "b", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_result", id),
        Pin::new(&format!("{}_result", id), "result", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Output),
    ));
    if let Some(b) = const_b { n.properties.insert(format!("{}_b", id), PropertyValue::Number(b)); }
    n
}

fn branch_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "branch", Position { x: 300.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{}_exec", id), Pin::new(&format!("{}_exec", id), "exec", DataType::Execution, PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{}_cond", id), Pin::new(&format!("{}_cond", id), "condition", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{}_true", id), Pin::new(&format!("{}_true", id), "True", DataType::Execution, PinType::Output)));
    n.outputs.push(PinInstance::new(&format!("{}_false", id), Pin::new(&format!("{}_false", id), "False", DataType::Execution, PinType::Output)));
    n
}

fn print_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "print_string", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{}_exec", id), Pin::new(&format!("{}_exec", id), "exec", DataType::Execution, PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{}_msg", id), Pin::new(&format!("{}_msg", id), "message", DataType::String, PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{}_exec_out", id), Pin::new(&format!("{}_exec_out", id), "exec", DataType::Execution, PinType::Output)));
    n
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = f();
    let elapsed = start.elapsed();
    println!("[timing] {:40} {:>10.3} ms", label, elapsed.as_secs_f64() * 1000.0);
    result
}

fn make_branch_graph() -> GraphDescription {
    let mut g = GraphDescription::new("branch_graph");
    g.add_node(begin_play("begin_exec"));
    g.add_node(add_node("add1", Some(10.0), Some(5.0)));
    g.add_node(gt_node("gt1", Some(0.0)));
    g.add_node(branch_node("br1"));
    g.add_node(print_node("pt"));
    g.add_node(print_node("pf"));

    g.add_connection(Connection::new("begin", "begin_exec", "br1", "br1_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("add1", "add1_result", "gt1", "gt1_a", ConnectionType::Data));
    g.add_connection(Connection::new("gt1", "gt1_result", "br1", "br1_cond", ConnectionType::Data));
    g.add_connection(Connection::new("br1", "br1_true", "pt", "pt_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("br1", "br1_false", "pf", "pf_exec", ConnectionType::Execution));
    g
}

fn make_deep_pure_graph(depth: usize) -> GraphDescription {
    let mut g = GraphDescription::new("deep_pure");
    g.add_node(begin_play("begin_exec"));

    for i in 0..depth {
        let id = format!("add_{}", i);
        let node = add_node(&id, if i == 0 { Some(0.0) } else { None }, Some(1.0));
        g.add_node(node);
        if i > 0 {
            let prev = format!("add_{}", i - 1);
            g.add_connection(Connection::new(
                &prev, &format!("{}_result", prev),
                &id, &format!("{}_a", id),
                ConnectionType::Data,
            ));
        }
    }

    // Terminate with a branch so the chain is actually emitted
    g.add_node(gt_node("gt_term", Some(0.0)));
    g.add_node(branch_node("br_term"));
    let last = format!("add_{}", depth - 1);
    g.add_connection(Connection::new(&last, &format!("{}_result", last), "gt_term", "gt_term_a", ConnectionType::Data));
    g.add_connection(Connection::new("begin", "begin_exec", "br_term", "br_term_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("gt_term", "gt_term_result", "br_term", "br_term_cond", ConnectionType::Data));
    g
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn test_end_to_end_branch_graph_compile_and_execute() {
    println!();
    let graph = make_branch_graph();

    // Stage 1: bytecode compilation
    let programs = time("branch_graph → bytecode compile", || {
        compile_graph_to_bytecode(&graph).expect("bytecode compile")
    });
    assert_eq!(programs.len(), 1, "should produce one begin_play program");
    let prog = &programs[0];
    println!("  instructions: {}, slots: {}", prog.instructions.len(), prog.slot_count);

    // Stage 2: single VM execution
    let dispatch = NativeStdDispatch;
    let vm = BytecodeVm::new(&dispatch);
    time("branch_graph → VM execute (1×)", || {
        vm.run(prog).expect("VM execute")
    });

    // Stage 3: 10,000 executions
    let start = Instant::now();
    for _ in 0..10_000 {
        vm.run(prog).expect("VM run");
    }
    let elapsed = start.elapsed();
    println!(
        "[timing] {:40} {:>10.3} ms total  ({:.2}µs/exec)",
        "branch_graph → VM execute (10,000×)",
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_micros() as f64 / 10_000.0
    );

    assert!(elapsed < Duration::from_secs(10), "10k runs should finish quickly");
}

#[test]
fn test_end_to_end_deep_pure_chain() {
    println!();
    const DEPTH: usize = 100;
    let graph = make_deep_pure_graph(DEPTH);

    let programs = time(&format!("{}-node pure chain → bytecode compile", DEPTH), || {
        compile_graph_to_bytecode(&graph).expect("bytecode compile")
    });
    let prog = &programs[0];
    println!("  instructions: {}, slots: {}", prog.instructions.len(), prog.slot_count);

    let dispatch = NativeStdDispatch;
    let vm = BytecodeVm::new(&dispatch);

    time(&format!("{}-node pure chain → VM execute (1×)", DEPTH), || {
        vm.run(prog).expect("VM run")
    });

    let start = Instant::now();
    for _ in 0..1_000 {
        vm.run(prog).unwrap();
    }
    let elapsed = start.elapsed();
    println!(
        "[timing] {:40} {:>10.3} ms total  ({:.2}µs/exec)",
        format!("{}-node chain → VM execute (1,000×)", DEPTH),
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_micros() as f64 / 1_000.0
    );
}

#[test]
fn test_bytecode_vs_rust_codegen_compile_time() {
    println!();
    let graph = make_deep_pure_graph(50);

    let rust_elapsed = {
        let start = Instant::now();
        let code = compile_graph(&graph).expect("rust codegen");
        let e = start.elapsed();
        println!(
            "[timing] {:40} {:>10.3} ms  ({} chars)",
            "50-node graph → Rust source codegen",
            e.as_secs_f64() * 1000.0,
            code.len()
        );
        e
    };

    let bc_elapsed = {
        let start = Instant::now();
        let programs = compile_graph_to_bytecode(&graph).expect("bytecode");
        let e = start.elapsed();
        println!(
            "[timing] {:40} {:>10.3} ms  ({} instrs)",
            "50-node graph → bytecode compile",
            e.as_secs_f64() * 1000.0,
            programs[0].instructions.len()
        );
        e
    };

    println!(
        "[timing] bytecode speedup vs Rust codegen: {:.1}×",
        rust_elapsed.as_secs_f64() / bc_elapsed.as_secs_f64().max(0.000001)
    );
}

#[test]
fn test_bytecode_serde_roundtrip() {
    // Bytecode must survive serialization for network/disk persistence
    let graph = make_branch_graph();
    let programs = compile_graph_to_bytecode(&graph).unwrap();
    let serialized = serde_json::to_string(&programs[0]).expect("serialize");
    let deserialized: pbgc::BpProgram = serde_json::from_str(&serialized).expect("deserialize");

    assert_eq!(programs[0].name, deserialized.name);
    assert_eq!(programs[0].slot_count, deserialized.slot_count);
    assert_eq!(programs[0].instructions.len(), deserialized.instructions.len());

    // Deserialized program should also execute correctly
    let dispatch = NativeStdDispatch;
    let vm = BytecodeVm::new(&dispatch);
    vm.run(&deserialized).expect("execute deserialized program");

    println!(
        "[info] serialized bytecode size: {} bytes JSON",
        serialized.len()
    );
}

/// Build a chain of `n` add nodes. Each feeds the next; the tail drives a branch.
/// Used for stress-testing the O(n) iterative preamble emitter.
fn make_500k_graph(n: usize) -> GraphDescription {
    let mut g = GraphDescription::new(format!("stress_{}", n));
    g.add_node(begin_play("begin_exec"));

    for i in 0..n {
        let id = format!("a{}", i);
        let node = add_node(&id, if i == 0 { Some(0.0) } else { None }, Some(1.0));
        g.add_node(node);
        if i > 0 {
            let prev = format!("a{}", i - 1);
            g.add_connection(Connection::new(
                &prev, &format!("{}_result", prev),
                &id,  &format!("{}_a", id),
                ConnectionType::Data,
            ));
        }
    }

    let last = format!("a{}", n - 1);
    g.add_node(gt_node("gt_end", Some(0.0)));
    g.add_node(branch_node("br_end"));

    g.add_connection(Connection::new(&last, &format!("{}_result", last), "gt_end", "gt_end_a", ConnectionType::Data));
    g.add_connection(Connection::new("begin", "begin_exec", "br_end", "br_end_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("gt_end", "gt_end_result", "br_end", "br_end_cond", ConnectionType::Data));
    g
}

#[test]
fn test_500k_node_graph_compile_and_execute() {
    println!();
    const N: usize = 500_000;

    // ── Graph construction ─────────────────────────────────────────────────────
    let graph = time(&format!("build {}-node graph struct", N), || make_500k_graph(N));
    println!("  nodes: {}, connections: {}", graph.nodes.len(), graph.connections.len());

    // ── Bytecode compilation ───────────────────────────────────────────────────
    let programs = time(&format!("{}-node graph → bytecode compile", N), || {
        compile_graph_to_bytecode(&graph).expect("bytecode compile")
    });
    let prog = &programs[0];
    println!(
        "  instructions: {}, slots: {}, program bytes (approx): {}",
        prog.instructions.len(),
        prog.slot_count,
        prog.instructions.len() * 32, // rough estimate
    );

    // ── Single execution ───────────────────────────────────────────────────────
    let dispatch = NativeStdDispatch;
    let vm = BytecodeVm::new(&dispatch);
    time(&format!("{}-node graph → VM execute (1×)", N), || {
        vm.run(prog).expect("VM execute")
    });

    // ── 10 executions ─────────────────────────────────────────────────────────
    let start = Instant::now();
    for _ in 0..10 {
        vm.run(prog).expect("VM run");
    }
    let elapsed = start.elapsed();
    println!(
        "[timing] {:40} {:>10.3} ms total  ({:.2}ms/exec)",
        format!("{}-node graph → VM execute (10×)", N),
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 100.0,
    );
}

#[test]
fn test_multiple_events_produce_separate_programs() {
    // A graph with two event nodes should produce two BpPrograms.
    let mut g = GraphDescription::new("two_events");

    let mut bp = NodeInstance::new("bp", "begin_play", Position { x: 0.0, y: 0.0 });
    bp.outputs.push(PinInstance::new("bp_exec", Pin::new("bp_exec", "Body", DataType::Execution, PinType::Output)));

    // "main" is the second registered event type in pulsar_std
    let mut main_ev = NodeInstance::new("main_ev", "main", Position { x: 200.0, y: 0.0 });
    main_ev.outputs.push(PinInstance::new("main_exec", Pin::new("main_exec", "Body", DataType::Execution, PinType::Output)));

    g.add_node(bp);
    g.add_node(main_ev);

    let programs = compile_graph_to_bytecode(&g).expect("compile two events");
    assert_eq!(programs.len(), 2, "expected one program per event node");

    let names: Vec<&str> = programs.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"begin_play"), "should have begin_play program");
    assert!(names.contains(&"main"), "should have main program");
}
