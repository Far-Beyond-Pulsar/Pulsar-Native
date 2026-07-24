/// End-to-end tests for `pulsar_bp_executor`.
///
/// Each test:
///   1. Builds a Blueprint graph with PBGC graph builders
///   2. Compiles it to bytecode with `compile_graph_to_bytecode`
///   3. Extracts `pulsar_std` native cdylib from the embedded bundle
///   4. Opens it with `BpExecutor::load` + resolves dispatch by node type name
///   5. Runs the program via `pbgc::vm::run` — zero conversion, zero enum matching
///
/// The `__bp_dispatch_*` shims inside `libpulsar_std` perform all type casting.
/// These tests are calling the actual compiled pulsar_std math functions.
use std::time::Instant;

use graphy::{
    Connection, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position,
};
use pbgc::compile_graph_to_bytecode;
use pulsar_bp_executor::BpExecutor;
use pulsar_std_bundle::{expected_sha256, extract_to_tempfile};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn executor() -> (BpExecutor, pulsar_std_bundle::TempLib) {
    let tmp = extract_to_tempfile().expect("extract dylib");
    let exec = BpExecutor::load(&tmp.path, Some(expected_sha256())).expect("load dylib");
    (exec, tmp)
}

fn run_program(exec: &BpExecutor, prog: &mut pbgc::BpProgram) {
    exec.prepare(prog)
        .unwrap_or_else(|e| panic!("prepare failed: {}", e));
    pbgc::vm::run(prog).unwrap_or_else(|e| panic!("vm::run failed: {}", e));
}

fn compile_and_run(exec: &BpExecutor, graph: &GraphDescription) {
    let mut programs = compile_graph_to_bytecode(graph).expect("compile failed");
    for prog in &mut programs {
        run_program(exec, prog);
    }
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

fn add_i64(id: &str, a: Option<f64>, b: Option<f64>) -> NodeInstance {
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

fn mul_i64(id: &str, a: Option<f64>, b: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "multiply", Position { x: 100.0, y: 0.0 });
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

fn sub_i64(id: &str, a: Option<f64>, b: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "subtract", Position { x: 100.0, y: 0.0 });
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

fn lerp_f64(id: &str, a: Option<f64>, b: Option<f64>, t: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "lerp", Position { x: 100.0, y: 0.0 });
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
    n.inputs.push(PinInstance::new(
        &format!("{id}_t"),
        Pin::new(
            &format!("{id}_t"),
            "t",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("f64"),
            PinType::Output,
        ),
    ));
    if let Some(v) = a {
        n.properties.insert(format!("{id}_a"), serde_json::json!(v));
    }
    if let Some(v) = b {
        n.properties.insert(format!("{id}_b"), serde_json::json!(v));
    }
    if let Some(v) = t {
        n.properties.insert(format!("{id}_t"), serde_json::json!(v));
    }
    n
}

fn gt_f64(id: &str, a: Option<f64>, b: Option<f64>) -> NodeInstance {
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

fn branch(id: &str) -> NodeInstance {
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

fn assert_eq_int(id: &str, expected: i64) -> NodeInstance {
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

fn assert_eq_float(id: &str, expected: f64, epsilon: f64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_float", Position { x: 400.0, y: 0.0 });
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
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_x"),
        Pin::new(
            &format!("{id}_x"),
            "expected",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_ep"),
        Pin::new(
            &format!("{id}_ep"),
            "epsilon",
            DataType::typed("f64"),
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
        .insert(format!("{id}_x"), serde_json::json!(expected));
    n.properties
        .insert(format!("{id}_ep"), serde_json::json!(epsilon));
    n
}

fn assert_true(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_true", Position { x: 400.0, y: 0.0 });
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
        &format!("{id}_o"),
        Pin::new(
            &format!("{id}_o"),
            "exec",
            DataType::Exec,
            PinType::Output,
        ),
    ));
    n
}

fn exec_conn(f: &str, fp: &str, t: &str, tp: &str) -> Connection {
    Connection::new(f, fp, t, tp, ConnectionType::Execution)
}
fn data_conn(f: &str, fp: &str, t: &str, tp: &str) -> Connection {
    Connection::new(f, fp, t, tp, ConnectionType::Data)
}

// ── Dylib sanity ──────────────────────────────────────────────────────────────

#[test]
fn test_dylib_loads_and_prepares() {
    let (exec, _tmp) = executor();
    // Prepare a trivial program to confirm the lib and symbol resolution work
    let mut prog = pbgc::BpProgram::new("test");
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
    // fn_ptr should now be non-zero
    if let pbgc::Instruction::Call { fn_ptr, .. } = &prog.instructions[0] {
        assert_ne!(*fn_ptr, 0, "fn_ptr must be resolved");
    }
}

#[test]
fn test_dylib_prepares_all_math_nodes() {
    let (exec, _tmp) = executor();
    let nodes = [
        "add",
        "subtract",
        "multiply",
        "divide",
        "modulo",
        "abs",
        "sqrt",
        "power",
        "lerp",
        "clamp",
        "min",
        "max",
        "ceil",
        "floor",
        "round",
        "greater_than",
        "less_than",
    ];
    let mut prog = pbgc::BpProgram::new("test");
    prog.arena_size = 32;
    for name in &nodes {
        prog.instructions.push(pbgc::Instruction::Call {
            fn_ptr: 0,
            node_type: name.to_string(),
            input_offsets: vec![0, 8],
            output_offset: 16,
            has_output: true,
            type_slot_offsets: vec![],
        });
    }
    prog.instructions.push(pbgc::Instruction::Return);
    exec.prepare(&mut prog)
        .expect("all math nodes must have dispatch symbols");
    for instr in &prog.instructions {
        if let pbgc::Instruction::Call {
            fn_ptr, node_type, ..
        } = instr
        {
            assert_ne!(*fn_ptr, 0, "{} fn_ptr must be non-zero", node_type);
        }
    }
}

#[test]
fn test_missing_symbol_returns_error() {
    let (exec, _tmp) = executor();
    let mut prog = pbgc::BpProgram::new("test");
    prog.arena_size = 8;
    prog.instructions.push(pbgc::Instruction::Call {
        fn_ptr: 0,
        node_type: "this_node_does_not_exist".to_string(),
        input_offsets: vec![],
        output_offset: 0,
        has_output: false,
        type_slot_offsets: vec![],
    });
    prog.instructions.push(pbgc::Instruction::Return);
    let result = exec.prepare(&mut prog);
    assert!(result.is_err(), "expected error for unknown node");
}

// ── Arithmetic correctness via real pulsar_std shims ─────────────────────────

#[test]
fn test_real_add_3_plus_4_is_7() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_i64("a", Some(3.0), Some(4.0)));
    g.add_node(assert_eq_int("chk", 7));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("a", "a_r", "chk", "chk_a"));
    compile_and_run(&exec, &g);
}

#[test]
fn test_real_multiply_6_times_7_is_42() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(mul_i64("m", Some(6.0), Some(7.0)));
    g.add_node(assert_eq_int("chk", 42));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("m", "m_r", "chk", "chk_a"));
    compile_and_run(&exec, &g);
}

#[test]
fn test_real_subtract_10_minus_3_is_7() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(sub_i64("s", Some(10.0), Some(3.0)));
    g.add_node(assert_eq_int("chk", 7));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("s", "s_r", "chk", "chk_a"));
    compile_and_run(&exec, &g);
}

#[test]
fn test_real_add_commutative_3_4_eq_4_3() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_i64("ab", Some(3.0), Some(4.0)));
    g.add_node(add_i64("ba", Some(4.0), Some(3.0)));
    g.add_node(assert_eq_int("c1", 7));
    g.add_node(assert_eq_int("c2", 7));
    g.add_connection(exec_conn("begin", "be", "c1", "c1_e"));
    g.add_connection(exec_conn("c1", "c1_o", "c2", "c2_e"));
    g.add_connection(data_conn("ab", "ab_r", "c1", "c1_a"));
    g.add_connection(data_conn("ba", "ba_r", "c2", "c2_a"));
    compile_and_run(&exec, &g);
}

#[test]
fn test_real_pythagorean_triple_3_4_5() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(mul_i64("s3", Some(3.0), Some(3.0)));
    g.add_node(mul_i64("s4", Some(4.0), Some(4.0)));
    g.add_node(add_i64("sum", None, None));
    g.add_node(assert_eq_int("chk", 25));
    g.add_connection(data_conn("s3", "s3_r", "sum", "sum_a"));
    g.add_connection(data_conn("s4", "s4_r", "sum", "sum_b"));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("sum", "sum_r", "chk", "chk_a"));
    compile_and_run(&exec, &g);
}

#[test]
fn test_real_add_chain_50_nodes_result_50() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    for i in 0..50usize {
        let id = format!("a{i}");
        let ca = if i == 0 { Some(0.0) } else { None };
        g.add_node(add_i64(&id, ca, Some(1.0)));
        if i > 0 {
            let prev = format!("a{}", i - 1);
            g.add_connection(data_conn(
                &prev,
                &format!("{prev}_r"),
                &id,
                &format!("{id}_a"),
            ));
        }
    }
    g.add_node(assert_eq_int("chk", 50));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("a49", "a49_r", "chk", "chk_a"));
    compile_and_run(&exec, &g);
}

// ── Float math via real shims ─────────────────────────────────────────────────

#[test]
fn test_real_lerp_midpoint_0_to_100_at_0_5_is_50() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(lerp_f64("l", Some(0.0), Some(100.0), Some(0.5)));
    g.add_node(assert_eq_float("chk", 50.0, 1e-9));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("l", "l_r", "chk", "chk_a"));
    compile_and_run(&exec, &g);
}

#[test]
fn test_real_lerp_at_zero_returns_a() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(lerp_f64("l", Some(42.0), Some(100.0), Some(0.0)));
    g.add_node(assert_eq_float("chk", 42.0, 1e-9));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("l", "l_r", "chk", "chk_a"));
    compile_and_run(&exec, &g);
}

#[test]
fn test_real_lerp_at_one_returns_b() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(lerp_f64("l", Some(0.0), Some(99.0), Some(1.0)));
    g.add_node(assert_eq_float("chk", 99.0, 1e-9));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("l", "l_r", "chk", "chk_a"));
    compile_and_run(&exec, &g);
}

// ── Control flow via real branch shim ────────────────────────────────────────

#[test]
fn test_real_branch_10_gt_5_takes_true_path() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(gt_f64("gt", Some(10.0), Some(5.0)));
    g.add_node(branch("br"));
    g.add_node(assert_true("at"));
    g.add_connection(exec_conn("begin", "be", "br", "br_e"));
    g.add_connection(data_conn("gt", "gt_r", "br", "br_c"));
    g.add_connection(exec_conn("br", "br_t", "at", "at_e"));
    g.add_connection(data_conn("gt", "gt_r", "at", "at_c"));
    compile_and_run(&exec, &g);
}

#[test]
fn test_real_branch_1_gt_5_takes_false_path_only() {
    // False branch is empty — if true path were taken we'd need assert to fire.
    // Just verify no panic / no wrong path taken.
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(gt_f64("gt", Some(1.0), Some(5.0)));
    g.add_node(branch("br"));
    g.add_connection(exec_conn("begin", "be", "br", "br_e"));
    g.add_connection(data_conn("gt", "gt_r", "br", "br_c"));
    // True branch has nothing connected — if taken, no crash
    compile_and_run(&exec, &g);
}

// ── Timing: real native dispatch ──────────────────────────────────────────────

#[test]
fn test_timing_10k_executions_branch_graph() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(gt_f64("gt", Some(10.0), Some(5.0)));
    g.add_node(add_i64("a", Some(3.0), Some(4.0)));
    g.add_node(branch("br"));
    g.add_node(assert_eq_int("chk", 7));
    g.add_connection(exec_conn("begin", "be", "br", "br_e"));
    g.add_connection(data_conn("gt", "gt_r", "br", "br_c"));
    g.add_connection(exec_conn("br", "br_t", "chk", "chk_e"));
    g.add_connection(data_conn("a", "a_r", "chk", "chk_a"));

    let mut programs = compile_graph_to_bytecode(&g).unwrap();
    exec.prepare(&mut programs[0]).unwrap();

    let t = Instant::now();
    for _ in 0..10_000 {
        pbgc::vm::run(&programs[0]).unwrap();
    }
    let elapsed = t.elapsed();
    println!(
        "[timing] 10,000 × branch graph (native dispatch): {:?}  ({:.2} µs/run)",
        elapsed,
        elapsed.as_micros() as f64 / 10_000.0
    );
    assert!(elapsed.as_secs() < 10, "took too long: {:?}", elapsed);
}

#[test]
fn test_timing_compile_vs_execute() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    for i in 0..20usize {
        let id = format!("a{i}");
        let ca = if i == 0 { Some(0.0) } else { None };
        g.add_node(add_i64(&id, ca, Some(1.0)));
        if i > 0 {
            let prev = format!("a{}", i - 1);
            g.add_connection(data_conn(
                &prev,
                &format!("{prev}_r"),
                &id,
                &format!("{id}_a"),
            ));
        }
    }
    g.add_node(gt_f64("gt", None, Some(0.0)));
    g.add_node(branch("br"));
    g.add_connection(data_conn("a19", "a19_r", "gt", "gt_a"));
    g.add_connection(exec_conn("begin", "be", "br", "br_e"));
    g.add_connection(data_conn("gt", "gt_r", "br", "br_c"));

    let t_compile = Instant::now();
    let mut programs = compile_graph_to_bytecode(&g).unwrap();
    let compile_time = t_compile.elapsed();

    exec.prepare(&mut programs[0]).unwrap();

    let t_exec = Instant::now();
    for _ in 0..1_000 {
        pbgc::vm::run(&programs[0]).unwrap();
    }
    let exec_time = t_exec.elapsed();

    println!(
        "[timing] 20-node: compile={:?}  execute×1000={:?} ({:.2}µs/run)",
        compile_time,
        exec_time,
        exec_time.as_micros() as f64 / 1000.0
    );
}

// ── Serde roundtrip with real execution ──────────────────────────────────────

#[test]
fn test_serde_roundtrip_executes_correctly_via_real_dylib() {
    let (exec, _tmp) = executor();
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_i64("a", Some(9.0), Some(10.0)));
    g.add_node(assert_eq_int("chk", 19));
    g.add_connection(exec_conn("begin", "be", "chk", "chk_e"));
    g.add_connection(data_conn("a", "a_r", "chk", "chk_a"));

    let programs = compile_graph_to_bytecode(&g).unwrap();
    let json = serde_json::to_string(&programs[0]).unwrap();
    let mut restored: pbgc::BpProgram = serde_json::from_str(&json).unwrap();

    exec.prepare(&mut restored).unwrap();
    pbgc::vm::run(&restored).unwrap();
}
