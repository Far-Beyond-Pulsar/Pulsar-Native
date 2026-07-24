/// Integration tests for pulsar_std_bundle — native cdylib edition.
///
/// Verifies that the embedded native dylib:
///   1. Contains valid bytes (non-empty)
///   2. Can be extracted to a temp file
///   3. Exports `__bp_dispatch_*` symbols that execute correctly via pbgc::vm::run
///   4. Roundtrips through JSON serialization
///   5. Handles large graphs without issue (500k-node stress)
use std::time::Instant;

use graphy::{
    Connection, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position,
};
use pbgc::{compile_graph, compile_graph_to_bytecode, BpProgram};
use pulsar_bp_executor::BpExecutor;
use pulsar_std_bundle::{
    expected_sha256, extract_to_tempfile, PULSAR_STD_LIB_BYTES, PULSAR_STD_LIB_EXT,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn executor() -> (BpExecutor, pulsar_std_bundle::TempLib) {
    let tmp = extract_to_tempfile().expect("extract native lib");
    let exec = BpExecutor::load(&tmp.path, Some(expected_sha256())).expect("load native lib");
    (exec, tmp)
}

fn run_program(exec: &BpExecutor, prog: &mut BpProgram) {
    exec.prepare(prog).expect("prepare");
    pbgc::vm::run(prog).expect("vm run");
}

fn compile_and_run(exec: &BpExecutor, g: &GraphDescription) {
    let mut progs = compile_graph_to_bytecode(g).expect("compile");
    for p in &mut progs {
        run_program(exec, p);
    }
}

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    println!(
        "[timing] {:45} {:>10.3} ms",
        label,
        t.elapsed().as_secs_f64() * 1000.0
    );
    r
}

// ── Graph builders ────────────────────────────────────────────────────────────

fn begin(pin: &str) -> NodeInstance {
    let mut n = NodeInstance::new("begin", "begin_play", Position { x: 0.0, y: 0.0 });
    n.outputs.push(PinInstance::new(
        pin,
        Pin::new(pin, "Body", DataType::Exec, PinType::Output),
    ));
    n
}
fn add_node(id: &str, a: Option<f64>, b: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "add", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "a",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_b"),
        Pin::new(
            &format!("{id}_b"),
            "b",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("i64"),
            PinType::Output,
        ),
    ));
    if let Some(v) = a {
        n.properties.insert(format!("{id}_a"), serde_json::json!(v));
    }
    if let Some(v) = b {
        n.properties.insert(format!("{id}_b"), serde_json::json!(v));
    }
    n
}
fn gt_node(id: &str, a: Option<f64>, b: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "greater_than", Position { x: 200.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "a",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_b"),
        Pin::new(
            &format!("{id}_b"),
            "b",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("bool"),
            PinType::Output,
        ),
    ));
    if let Some(v) = a {
        n.properties.insert(format!("{id}_a"), serde_json::json!(v));
    }
    if let Some(v) = b {
        n.properties.insert(format!("{id}_b"), serde_json::json!(v));
    }
    n
}
fn branch_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "branch", Position { x: 300.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_e"),
        Pin::new(
            &format!("{id}_e"),
            "exec",
            DataType::Exec,
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_c"),
        Pin::new(
            &format!("{id}_c"),
            "condition",
            DataType::typed("bool"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_t"),
        Pin::new(
            &format!("{id}_t"),
            "True",
            DataType::Exec,
            PinType::Output,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_f"),
        Pin::new(
            &format!("{id}_f"),
            "False",
            DataType::Exec,
            PinType::Output,
        ),
    ));
    n
}
fn assert_eq_int_node(id: &str, expected: i64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_int", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_e"),
        Pin::new(
            &format!("{id}_e"),
            "exec",
            DataType::Exec,
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "actual",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_x"),
        Pin::new(
            &format!("{id}_x"),
            "expected",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_o"),
        Pin::new(
            &format!("{id}_o"),
            "exec",
            DataType::Exec,
            PinType::Output,
        ),
    ));
    n.properties
        .insert(format!("{id}_x"), serde_json::json!(expected as f64));
    n
}

fn make_deep_graph(n: usize) -> GraphDescription {
    let mut g = GraphDescription::new("deep");
    g.add_node(begin("be"));
    for i in 0..n {
        let id = format!("a{i}");
        let ca = if i == 0 { Some(0.0) } else { None };
        g.add_node(add_node(&id, ca, Some(1.0)));
        if i > 0 {
            let prev = format!("a{}", i - 1);
            g.add_connection(Connection::new(
                &prev,
                &format!("{prev}_r"),
                &id,
                &format!("{id}_a"),
                ConnectionType::Data,
            ));
        }
    }
    let last = format!("a{}", n - 1);
    g.add_node(gt_node("gt", None, Some(0.0)));
    g.add_node(branch_node("br"));
    g.add_connection(Connection::new(
        &last,
        &format!("{last}_r"),
        "gt",
        "gt_a",
        ConnectionType::Data,
    ));
    g.add_connection(Connection::new(
        "begin",
        "be",
        "br",
        "br_e",
        ConnectionType::Execution,
    ));
    g.add_connection(Connection::new(
        "gt",
        "gt_r",
        "br",
        "br_c",
        ConnectionType::Data,
    ));
    g
}

// ── Native lib validity ───────────────────────────────────────────────────────

#[test]
fn test_embedded_lib_is_non_empty() {
    assert!(
        !PULSAR_STD_LIB_BYTES.is_empty(),
        "embedded lib must not be empty"
    );
    println!(
        "[info] PULSAR_STD_LIB embedded: {} bytes ({:.1} KiB), ext={}",
        PULSAR_STD_LIB_BYTES.len(),
        PULSAR_STD_LIB_BYTES.len() as f64 / 1024.0,
        PULSAR_STD_LIB_EXT
    );
}

#[test]
fn test_lib_extracts_to_temp_and_loads() {
    let tmp = extract_to_tempfile().expect("extract");
    assert!(tmp.path.exists(), "temp file must exist");
    let exec = BpExecutor::load(&tmp.path, Some(expected_sha256())).expect("load");
    // Prepare a trivial program to confirm the lib is functional
    let mut prog = BpProgram::new("test");
    prog.arena_size = 24;
    prog.instructions.push(pbgc::Instruction::Call {
        fn_ptr: 0,
        node_type: "add".to_string(),
        input_offsets: vec![0, 8],
        output_offset: 16,
        has_output: true,
        type_slot_offsets: vec![],
    });
    prog.instructions.push(pbgc::Instruction::Return);
    exec.prepare(&mut prog).expect("add must be prepareable");
}

// ── Correctness via embedded native lib ──────────────────────────────────────

#[test]
fn test_end_to_end_add_3_plus_4_is_7() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_node("a", Some(3.0), Some(4.0)));
    g.add_node(assert_eq_int_node("chk", 7));
    g.add_connection(Connection::new(
        "begin",
        "be",
        "chk",
        "chk_e",
        ConnectionType::Execution,
    ));
    g.add_connection(Connection::new(
        "a",
        "a_r",
        "chk",
        "chk_a",
        ConnectionType::Data,
    ));
    compile_and_run(&exec, &g);
}

#[test]
fn test_end_to_end_branch_graph() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("branch");
    g.add_node(begin("be"));
    g.add_node(add_node("a", Some(10.0), Some(5.0)));
    g.add_node(gt_node("gt", None, Some(0.0)));
    g.add_node(branch_node("br"));
    g.add_connection(Connection::new(
        "a",
        "a_r",
        "gt",
        "gt_a",
        ConnectionType::Data,
    ));
    g.add_connection(Connection::new(
        "begin",
        "be",
        "br",
        "br_e",
        ConnectionType::Execution,
    ));
    g.add_connection(Connection::new(
        "gt",
        "gt_r",
        "br",
        "br_c",
        ConnectionType::Data,
    ));
    compile_and_run(&exec, &g);
}

#[test]
fn test_multiple_event_nodes_produce_separate_programs() {
    let mut g = GraphDescription::new("two_events");
    let mut bp = NodeInstance::new("bp", "begin_play", Position { x: 0.0, y: 0.0 });
    bp.outputs.push(PinInstance::new(
        "bp_e",
        Pin::new("bp_e", "Body", DataType::Exec, PinType::Output),
    ));
    let mut main_ev = NodeInstance::new("main_ev", "main", Position { x: 200.0, y: 0.0 });
    main_ev.outputs.push(PinInstance::new(
        "main_e",
        Pin::new("main_e", "Body", DataType::Exec, PinType::Output),
    ));
    g.add_node(bp);
    g.add_node(main_ev);
    let programs = compile_graph_to_bytecode(&g).expect("compile");
    assert_eq!(programs.len(), 2);
    let names: Vec<&str> = programs.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"begin_play"));
    assert!(names.contains(&"main"));
}

// ── Serde roundtrip ───────────────────────────────────────────────────────────

#[test]
fn test_serde_roundtrip_and_execution() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_node("a", Some(7.0), Some(8.0)));
    g.add_node(assert_eq_int_node("chk", 15));
    g.add_connection(Connection::new(
        "begin",
        "be",
        "chk",
        "chk_e",
        ConnectionType::Execution,
    ));
    g.add_connection(Connection::new(
        "a",
        "a_r",
        "chk",
        "chk_a",
        ConnectionType::Data,
    ));

    let programs = compile_graph_to_bytecode(&g).unwrap();
    let json = serde_json::to_string(&programs[0]).unwrap();
    let mut restored: BpProgram = serde_json::from_str(&json).unwrap();
    assert_eq!(programs[0].arena_size, restored.arena_size);
    run_program(&exec, &mut restored);
    println!("[info] serialized bytecode: {} bytes JSON", json.len());
}

// ── Timing ────────────────────────────────────────────────────────────────────

#[test]
fn test_timing_branch_graph_10k_runs() {
    println!();
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("branch");
    g.add_node(begin("be"));
    g.add_node(add_node("a", Some(3.0), Some(4.0)));
    g.add_node(gt_node("gt", None, Some(0.0)));
    g.add_node(branch_node("br"));
    g.add_node(assert_eq_int_node("chk", 7));
    g.add_connection(Connection::new(
        "a",
        "a_r",
        "gt",
        "gt_a",
        ConnectionType::Data,
    ));
    g.add_connection(Connection::new(
        "begin",
        "be",
        "br",
        "br_e",
        ConnectionType::Execution,
    ));
    g.add_connection(Connection::new(
        "gt",
        "gt_r",
        "br",
        "br_c",
        ConnectionType::Data,
    ));
    g.add_connection(Connection::new(
        "br",
        "br_t",
        "chk",
        "chk_e",
        ConnectionType::Execution,
    ));
    g.add_connection(Connection::new(
        "a",
        "a_r",
        "chk",
        "chk_a",
        ConnectionType::Data,
    ));

    let mut programs = time("branch graph → bytecode compile", || {
        compile_graph_to_bytecode(&g).unwrap()
    });
    println!(
        "  instructions: {}, arena: {} bytes",
        programs[0].instructions.len(),
        programs[0].arena_size
    );

    exec.prepare(&mut programs[0]).unwrap();

    time("branch graph → VM execute (1×)", || {
        pbgc::vm::run(&programs[0]).unwrap()
    });

    let t = Instant::now();
    for _ in 0..10_000 {
        pbgc::vm::run(&programs[0]).unwrap();
    }
    let elapsed = t.elapsed();
    println!(
        "[timing] {:45} {:>10.3} ms total  ({:.2} µs/run)",
        "branch graph → VM execute (10,000×)",
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_micros() as f64 / 10_000.0
    );
}

#[test]
fn test_timing_bytecode_vs_rust_codegen() {
    println!();
    let g = make_deep_graph(50);

    let t0 = Instant::now();
    let code = compile_graph(&g).unwrap();
    let rust_t = t0.elapsed();
    println!(
        "[timing] {:45} {:>10.3} ms  ({} chars)",
        "50-node → Rust codegen",
        rust_t.as_secs_f64() * 1000.0,
        code.len()
    );

    let t1 = Instant::now();
    let progs = compile_graph_to_bytecode(&g).unwrap();
    let bc_t = t1.elapsed();
    println!(
        "[timing] {:45} {:>10.3} ms  ({} instrs)",
        "50-node → bytecode compile",
        bc_t.as_secs_f64() * 1000.0,
        progs[0].instructions.len()
    );

    println!(
        "[timing] speedup: {:.1}×",
        rust_t.as_secs_f64() / bc_t.as_secs_f64().max(1e-9)
    );
}

#[test]
fn test_timing_100_node_chain() {
    println!();
    let (exec, _tmp) = executor();
    let g = make_deep_graph(100);

    let mut progs = time("100-node chain → bytecode compile", || {
        compile_graph_to_bytecode(&g).unwrap()
    });
    println!(
        "  instructions: {}, arena: {} bytes",
        progs[0].instructions.len(),
        progs[0].arena_size
    );

    exec.prepare(&mut progs[0]).unwrap();
    time("100-node chain → VM execute (1×)", || {
        pbgc::vm::run(&progs[0]).unwrap()
    });

    let t = Instant::now();
    for _ in 0..1_000 {
        pbgc::vm::run(&progs[0]).unwrap();
    }
    let elapsed = t.elapsed();
    println!(
        "[timing] {:45} {:>10.3} ms total  ({:.2} µs/run)",
        "100-node chain → VM execute (1,000×)",
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_micros() as f64 / 1_000.0
    );
}

#[test]
fn test_timing_500k_node_stress() {
    println!();
    let (exec, _tmp) = executor();

    let g = time("build 500k-node graph", || make_deep_graph(500_000));
    println!(
        "  nodes: {}, connections: {}",
        g.nodes.len(),
        g.connections.len()
    );

    let mut progs = time("500k-node → bytecode compile", || {
        compile_graph_to_bytecode(&g).unwrap()
    });
    println!(
        "  instructions: {}, arena: {} bytes",
        progs[0].instructions.len(),
        progs[0].arena_size
    );

    exec.prepare(&mut progs[0]).unwrap();
    time("500k-node → VM execute (1×)", || {
        pbgc::vm::run(&progs[0]).unwrap()
    });
}
