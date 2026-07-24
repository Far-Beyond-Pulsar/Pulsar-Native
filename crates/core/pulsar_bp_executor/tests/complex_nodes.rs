/// Tests for every dispatchable pulsar_std node that goes beyond basic add/multiply.
/// Each test builds a real Blueprint graph, compiles it to bytecode, runs it through
/// the native dylib dispatch, and asserts the correct answer via assert_eq_* nodes.
/// The executor resolves every fn pointer by name from the embedded dylib — no
/// manual dispatch table, no type annotations in the test itself.
use graphy::{
    Connection, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position,
};
use pbgc::compile_graph_to_bytecode;
use pulsar_bp_executor::BpExecutor;
use pulsar_std_bundle::{expected_sha256, extract_to_tempfile};

// ── Shared executor ───────────────────────────────────────────────────────────

fn exec() -> (BpExecutor, pulsar_std_bundle::TempLib) {
    let tmp = extract_to_tempfile().unwrap();
    let e = BpExecutor::load(&tmp.path, Some(expected_sha256())).unwrap();
    (e, tmp)
}

fn run(e: &BpExecutor, g: &GraphDescription) {
    let mut progs = compile_graph_to_bytecode(g).expect("compile");
    for p in &mut progs {
        e.prepare(p)
            .unwrap_or_else(|err| panic!("prepare: {}", err));
        pbgc::vm::run(p).unwrap_or_else(|err| panic!("run: {}", err));
    }
}

// ── Node builders ─────────────────────────────────────────────────────────────

fn begin() -> NodeInstance {
    let mut n = NodeInstance::new("begin", "begin_play", Position::default());
    n.outputs.push(PinInstance::new(
        "be",
        Pin::new("be", "Body", DataType::Exec, PinType::Output),
    ));
    n
}

fn pure_f64(id: &str, node_type: &str, param_names: &[&str], consts: &[f64]) -> NodeInstance {
    let mut n = NodeInstance::new(id, node_type, Position::default());
    for (i, &name) in param_names.iter().enumerate() {
        let pid = format!("{id}_{name}");
        n.inputs.push(PinInstance::new(
            &pid,
            Pin::new(
                &pid,
                name.to_string(),
                DataType::typed("f64"),
                PinType::Input,
            ),
        ));
        if let Some(&v) = consts.get(i) {
            n.properties.insert(pid, serde_json::json!(v));
        }
    }
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("f64"),
            PinType::Output,
        ),
    ));
    n
}

fn pure_i64(id: &str, node_type: &str, param_names: &[&str], consts: &[f64]) -> NodeInstance {
    let mut n = NodeInstance::new(id, node_type, Position::default());
    for (i, &name) in param_names.iter().enumerate() {
        let pid = format!("{id}_{name}");
        n.inputs.push(PinInstance::new(
            &pid,
            Pin::new(
                &pid,
                name.to_string(),
                DataType::typed("i64"),
                PinType::Input,
            ),
        ));
        if let Some(&v) = consts.get(i) {
            n.properties.insert(pid, serde_json::json!(v));
        }
    }
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("i64"),
            PinType::Output,
        ),
    ));
    n
}

fn pure_bool_out(
    id: &str,
    node_type: &str,
    param_names: &[(&str, &str)],
    consts: &[f64],
) -> NodeInstance {
    let mut n = NodeInstance::new(id, node_type, Position::default());
    for (i, &(name, ty)) in param_names.iter().enumerate() {
        let pid = format!("{id}_{name}");
        n.inputs.push(PinInstance::new(
            &pid,
            Pin::new(
                &pid,
                name.to_string(),
                DataType::typed(ty),
                PinType::Input,
            ),
        ));
        if let Some(&v) = consts.get(i) {
            n.properties.insert(pid, serde_json::json!(v));
        }
    }
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("bool"),
            PinType::Output,
        ),
    ));
    n
}

fn assert_eq_int(id: &str, expected: i64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_int", Position::default());
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

fn assert_eq_float(id: &str, expected: f64, eps: f64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_float", Position::default());
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
        .insert(format!("{id}_ep"), serde_json::json!(eps));
    n
}

fn assert_eq_f32(id: &str, expected: f32, eps: f32) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_f32", Position::default());
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
            DataType::typed("f32"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_x"),
        Pin::new(
            &format!("{id}_x"),
            "expected",
            DataType::typed("f32"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_ep"),
        Pin::new(
            &format!("{id}_ep"),
            "epsilon",
            DataType::typed("f32"),
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
    n.properties
        .insert(format!("{id}_ep"), serde_json::json!(eps as f64));
    n
}

fn assert_true(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_true", Position::default());
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

fn assert_false(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_false", Position::default());
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

fn e(f: &str, fp: &str, t: &str, tp: &str) -> Connection {
    Connection::new(f, fp, t, tp, ConnectionType::Execution)
}
fn d(f: &str, fp: &str, t: &str, tp: &str) -> Connection {
    Connection::new(f, fp, t, tp, ConnectionType::Data)
}

// wire: pure_node_result → assert exec chain
fn simple_f64_check(
    node_type: &str,
    params: &[&str],
    consts: &[f64],
    expected: f64,
    eps: f64,
) -> GraphDescription {
    let mut g = GraphDescription::new(node_type);
    g.add_node(begin());
    g.add_node(pure_f64("n", node_type, params, consts));
    g.add_node(assert_eq_float("chk", expected, eps));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("n", "n_r", "chk", "chk_a"));
    g
}

fn simple_i64_check(
    node_type: &str,
    params: &[&str],
    consts: &[f64],
    expected: i64,
) -> GraphDescription {
    let mut g = GraphDescription::new(node_type);
    g.add_node(begin());
    g.add_node(pure_i64("n", node_type, params, consts));
    g.add_node(assert_eq_int("chk", expected));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("n", "n_r", "chk", "chk_a"));
    g
}

// ── Trig — real pulsar_std sin/cos/tan ────────────────────────────────────────

#[test]
fn test_sin_0_is_0() {
    let (e, _) = exec();
    run(&e, &simple_f64_check("sin", &["angle"], &[0.0], 0.0, 1e-9));
}
#[test]
fn test_sin_pi_over_2_is_1() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("sin", &["angle"], &[std::f64::consts::FRAC_PI_2], 1.0, 1e-9),
    );
}
#[test]
fn test_cos_0_is_1() {
    let (e, _) = exec();
    run(&e, &simple_f64_check("cos", &["angle"], &[0.0], 1.0, 1e-9));
}
#[test]
fn test_cos_pi_is_minus_1() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("cos", &["angle"], &[std::f64::consts::PI], -1.0, 1e-9),
    );
}
#[test]
fn test_tan_0_is_0() {
    let (e, _) = exec();
    run(&e, &simple_f64_check("tan", &["angle"], &[0.0], 0.0, 1e-9));
}

// ── Angle conversion ──────────────────────────────────────────────────────────

#[test]
fn test_degrees_to_radians_180_is_pi() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "degrees_to_radians",
            &["degrees"],
            &[180.0],
            std::f64::consts::PI,
            1e-9,
        ),
    );
}
#[test]
fn test_radians_to_degrees_pi_is_180() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "radians_to_degrees",
            &["radians"],
            &[std::f64::consts::PI],
            180.0,
            1e-9,
        ),
    );
}
#[test]
fn test_roundtrip_degrees_radians() {
    // degrees_to_radians(radians_to_degrees(pi)) == pi
    let (exec, _) = exec();
    let mut g = GraphDescription::new("roundtrip");
    g.add_node(begin());
    g.add_node(pure_f64(
        "r2d",
        "radians_to_degrees",
        &["radians"],
        &[std::f64::consts::PI],
    ));
    g.add_node(pure_f64("d2r", "degrees_to_radians", &["degrees"], &[]));
    g.add_node(assert_eq_float("chk", std::f64::consts::PI, 1e-9));
    g.add_connection(d("r2d", "r2d_r", "d2r", "d2r_degrees"));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("d2r", "d2r_r", "chk", "chk_a"));
    run(&exec, &g);
}

// ── Rounding ──────────────────────────────────────────────────────────────────

#[test]
fn test_ceil_3_2_is_4() {
    let (e, _) = exec();
    run(&e, &simple_f64_check("ceil", &["value"], &[3.2], 4.0, 1e-9));
}
#[test]
fn test_ceil_neg_2_7() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("ceil", &["value"], &[-2.7], -2.0, 1e-9),
    );
}
#[test]
fn test_floor_3_9_is_3() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("floor", &["value"], &[3.9], 3.0, 1e-9),
    );
}
#[test]
fn test_floor_neg_2_1() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("floor", &["value"], &[-2.1], -3.0, 1e-9),
    );
}
#[test]
fn test_round_3_5_is_4() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("round", &["value"], &[3.5], 4.0, 1e-9),
    );
}
#[test]
fn test_round_neg_0_5() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("round", &["value"], &[-0.5], -1.0, 1e-9),
    );
}

// ── abs / sign ────────────────────────────────────────────────────────────────

#[test]
fn test_abs_negative() {
    let (e, _) = exec();
    run(&e, &simple_f64_check("abs", &["value"], &[-7.5], 7.5, 1e-9));
}
#[test]
fn test_abs_positive() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("abs", &["value"], &[3.14], 3.14, 1e-9),
    );
}
#[test]
fn test_sign_positive() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("sign", &["value"], &[99.9], 1.0, 1e-9),
    );
}
#[test]
fn test_sign_negative() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("sign", &["value"], &[-0.01], -1.0, 1e-9),
    );
}

// ── sqrt / power ──────────────────────────────────────────────────────────────

#[test]
fn test_sqrt_9_is_3() {
    let (e, _) = exec();
    run(&e, &simple_f64_check("sqrt", &["value"], &[9.0], 3.0, 1e-9));
}
#[test]
fn test_sqrt_2() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("sqrt", &["value"], &[2.0], 2f64.sqrt(), 1e-12),
    );
}
#[test]
fn test_power_2_cubed() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("power", &["base", "exponent"], &[2.0, 3.0], 8.0, 1e-9),
    );
}
#[test]
fn test_power_identity() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("power", &["base", "exponent"], &[42.0, 1.0], 42.0, 1e-9),
    );
}
#[test]
fn test_power_zero_exp() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("power", &["base", "exponent"], &[99.0, 0.0], 1.0, 1e-9),
    );
}
#[test]
fn test_sqrt_of_power_2() {
    // sqrt(x²) == x for x > 0
    let (exec, _) = exec();
    let x = 7.0f64;
    let mut g = GraphDescription::new("sqrt_pow");
    g.add_node(begin());
    g.add_node(pure_f64("p", "power", &["base", "exponent"], &[x, 2.0]));
    g.add_node(pure_f64("s", "sqrt", &["value"], &[]));
    g.add_node(assert_eq_float("chk", x, 1e-9));
    g.add_connection(d("p", "p_r", "s", "s_value"));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("s", "s_r", "chk", "chk_a"));
    run(&exec, &g);
}

// ── min / max ─────────────────────────────────────────────────────────────────

#[test]
fn test_min_3_5_is_3() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("min", &["a", "b"], &[3.0, 5.0], 3.0, 1e-9),
    );
}
#[test]
fn test_max_3_5_is_5() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("max", &["a", "b"], &[3.0, 5.0], 5.0, 1e-9),
    );
}
#[test]
fn test_min_equal_vals() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("min", &["a", "b"], &[7.0, 7.0], 7.0, 1e-9),
    );
}

// ── clamp ─────────────────────────────────────────────────────────────────────

#[test]
fn test_clamp_in_range() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "clamp",
            &["value", "min", "max"],
            &[5.0, 0.0, 10.0],
            5.0,
            1e-9,
        ),
    );
}
#[test]
fn test_clamp_below() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "clamp",
            &["value", "min", "max"],
            &[-3.0, 0.0, 10.0],
            0.0,
            1e-9,
        ),
    );
}
#[test]
fn test_clamp_above() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "clamp",
            &["value", "min", "max"],
            &[15.0, 0.0, 10.0],
            10.0,
            1e-9,
        ),
    );
}

// ── f32 nodes — smoothstep, clamp_to_range, map_range ─────────────────────────
// These use f32 params; dispatch shim reads as f32::from_bits(slot as u32)

#[test]
fn test_smoothstep_midpoint() {
    // smoothstep(0.0, 1.0, 0.5) = 0.5  (by definition)
    let (exec, _) = exec();
    let mut g = GraphDescription::new("smoothstep");
    g.add_node(begin());

    let mut n = NodeInstance::new("n", "smoothstep", Position::default());
    for name in &["edge0", "edge1", "x"] {
        let pid = format!("n_{name}");
        n.inputs.push(PinInstance::new(
            &pid,
            Pin::new(
                &pid,
                name.to_string(),
                DataType::typed("f32"),
                PinType::Input,
            ),
        ));
    }
    n.outputs.push(PinInstance::new(
        "n_r",
        Pin::new(
            "n_r",
            "result",
            DataType::typed("f32"),
            PinType::Output,
        ),
    ));
    n.properties
        .insert("n_edge0".to_string(), serde_json::json!(0.0));
    n.properties
        .insert("n_edge1".to_string(), serde_json::json!(1.0));
    n.properties
        .insert("n_x".to_string(), serde_json::json!(0.5));
    g.add_node(n);

    g.add_node(assert_eq_f32("chk", 0.5f32, 1e-6));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("n", "n_r", "chk", "chk_a"));
    run(&exec, &g);
}

#[test]
fn test_clamp_to_range_below_min() {
    // clamp_to_range(-5.0, 0.0, 10.0) = 0.0
    let (exec, _) = exec();
    let mut g = GraphDescription::new("clamp_to_range");
    g.add_node(begin());
    let mut n = NodeInstance::new("n", "clamp_to_range", Position::default());
    for name in &["value", "min", "max"] {
        let pid = format!("n_{name}");
        n.inputs.push(PinInstance::new(
            &pid,
            Pin::new(
                &pid,
                (*name).to_string(),
                DataType::typed("f32"),
                PinType::Input,
            ),
        ));
    }
    n.outputs.push(PinInstance::new(
        "n_r",
        Pin::new(
            "n_r",
            "result",
            DataType::typed("f32"),
            PinType::Output,
        ),
    ));
    n.properties
        .insert("n_value".to_string(), serde_json::json!(-5.0));
    n.properties
        .insert("n_min".to_string(), serde_json::json!(0.0));
    n.properties
        .insert("n_max".to_string(), serde_json::json!(10.0));
    g.add_node(n);
    g.add_node(assert_eq_f32("chk", 0.0f32, 1e-6));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("n", "n_r", "chk", "chk_a"));
    run(&exec, &g);
}

// ── Bitwise operations ────────────────────────────────────────────────────────

#[test]
fn test_bitwise_and() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("bitwise_and", &["a", "b"], &[12.0, 10.0], 8),
    );
}
#[test]
fn test_bitwise_or() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("bitwise_or", &["a", "b"], &[12.0, 10.0], 14),
    );
}
#[test]
fn test_bitwise_xor() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("bitwise_xor", &["a", "b"], &[12.0, 10.0], 6),
    );
}
#[test]
fn test_bitwise_not_zero() {
    let (e, _) = exec();
    run(&e, &simple_i64_check("bitwise_not", &["value"], &[0.0], -1));
}
#[test]
fn test_bit_shift_left() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("bit_shift_left", &["value", "bits"], &[1.0, 3.0], 8),
    );
}
#[test]
fn test_bit_shift_right() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("bit_shift_right", &["value", "bits"], &[8.0, 2.0], 2),
    );
}
#[test]
fn test_count_bits() {
    let (e, _) = exec();
    run(&e, &simple_i64_check("count_bits", &["value"], &[183.0], 6));
}
#[test]
fn test_get_bit_set() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("get_bit", &["value", "bit_index"], &[10.0, 1.0], 1),
    );
}
#[test]
fn test_get_bit_clear() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("get_bit", &["value", "bit_index"], &[10.0, 0.0], 0),
    );
}
#[test]
fn test_set_bit() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("set_bit", &["value", "bit_index"], &[8.0, 0.0], 9),
    );
}
#[test]
fn test_clear_bit() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("clear_bit", &["value", "bit_index"], &[15.0, 0.0], 14),
    );
}
#[test]
fn test_toggle_bit_on() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("toggle_bit", &["value", "bit_index"], &[10.0, 0.0], 11),
    );
}
#[test]
fn test_toggle_bit_off() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("toggle_bit", &["value", "bit_index"], &[11.0, 0.0], 10),
    );
}

// ── Logic / boolean ───────────────────────────────────────────────────────────

fn bool_check(
    node_type: &str,
    params: &[(&str, &str)],
    consts: &[f64],
    expect_true: bool,
) -> GraphDescription {
    let mut g = GraphDescription::new(node_type);
    g.add_node(begin());
    g.add_node(pure_bool_out("n", node_type, params, consts));
    let assert_node = if expect_true {
        assert_true("chk")
    } else {
        assert_false("chk")
    };
    g.add_node(assert_node);
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("n", "n_r", "chk", "chk_c"));
    g
}

#[test]
fn test_and_tt() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check("and", &[("a", "bool"), ("b", "bool")], &[1.0, 1.0], true),
    );
}
#[test]
fn test_and_tf() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check("and", &[("a", "bool"), ("b", "bool")], &[1.0, 0.0], false),
    );
}
#[test]
fn test_or_ff() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check("or", &[("a", "bool"), ("b", "bool")], &[0.0, 0.0], false),
    );
}
#[test]
fn test_or_tf() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check("or", &[("a", "bool"), ("b", "bool")], &[1.0, 0.0], true),
    );
}
#[test]
fn test_not_false() {
    let (e, _) = exec();
    run(&e, &bool_check("not", &[("value", "bool")], &[0.0], true));
}
#[test]
fn test_not_true() {
    let (e, _) = exec();
    run(&e, &bool_check("not", &[("value", "bool")], &[1.0], false));
}
#[test]
fn test_xor_tt() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check("xor", &[("a", "bool"), ("b", "bool")], &[1.0, 1.0], false),
    );
}
#[test]
fn test_xor_tf() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check("xor", &[("a", "bool"), ("b", "bool")], &[1.0, 0.0], true),
    );
}
#[test]
fn test_equals_same() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check("equals", &[("a", "i64"), ("b", "i64")], &[7.0, 7.0], true),
    );
}
#[test]
fn test_equals_diff() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check("equals", &[("a", "i64"), ("b", "i64")], &[7.0, 8.0], false),
    );
}
#[test]
fn test_not_equals() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check(
            "not_equals",
            &[("a", "i64"), ("b", "i64")],
            &[3.0, 4.0],
            true,
        ),
    );
}
#[test]
fn test_greater_equal_eq() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check(
            "greater_equal",
            &[("a", "i64"), ("b", "i64")],
            &[5.0, 5.0],
            true,
        ),
    );
}
#[test]
fn test_greater_equal_gt() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check(
            "greater_equal",
            &[("a", "i64"), ("b", "i64")],
            &[6.0, 5.0],
            true,
        ),
    );
}
#[test]
fn test_less_equal_eq() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check(
            "less_equal",
            &[("a", "i64"), ("b", "i64")],
            &[5.0, 5.0],
            true,
        ),
    );
}
#[test]
fn test_is_between() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check(
            "is_between",
            &[("value", "f64"), ("a", "f64"), ("b", "f64")],
            &[5.0, 0.0, 10.0],
            true,
        ),
    );
}
#[test]
fn test_is_between_out() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check(
            "is_between",
            &[("value", "f64"), ("a", "f64"), ("b", "f64")],
            &[15.0, 0.0, 10.0],
            false,
        ),
    );
}
#[test]
fn test_nearly_equal() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check(
            "nearly_equal",
            &[("a", "f64"), ("b", "f64"), ("tolerance", "f64")],
            &[1.0, 1.0001, 0.001],
            true,
        ),
    );
}
#[test]
fn test_nearly_not_equal() {
    let (e, _) = exec();
    run(
        &e,
        &bool_check(
            "nearly_equal",
            &[("a", "f64"), ("b", "f64"), ("tolerance", "f64")],
            &[1.0, 2.0, 0.001],
            false,
        ),
    );
}

// ── select_bool / select_number ───────────────────────────────────────────────

#[test]
fn test_select_number_true() {
    // select_number(true, 10.0, 20.0) == 10.0
    let (exec, _) = exec();
    let mut g = GraphDescription::new("select");
    g.add_node(begin());
    let mut n = NodeInstance::new("n", "select_number", Position::default());
    n.inputs.push(PinInstance::new(
        "n_cond",
        Pin::new(
            "n_cond",
            "condition",
            DataType::typed("bool"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        "n_a",
        Pin::new(
            "n_a",
            "a",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        "n_b",
        Pin::new(
            "n_b",
            "b",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        "n_r",
        Pin::new(
            "n_r",
            "result",
            DataType::typed("f64"),
            PinType::Output,
        ),
    ));
    n.properties
        .insert("n_cond".to_string(), serde_json::json!(1.0));
    n.properties
        .insert("n_a".to_string(), serde_json::json!(10.0));
    n.properties
        .insert("n_b".to_string(), serde_json::json!(20.0));
    g.add_node(n);
    g.add_node(assert_eq_float("chk", 10.0, 1e-9));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("n", "n_r", "chk", "chk_a"));
    run(&exec, &g);
}

#[test]
fn test_select_number_false() {
    // select_number(false, 10.0, 20.0) == 20.0
    let (exec, _) = exec();
    let mut g = GraphDescription::new("select_false");
    g.add_node(begin());
    let mut n = NodeInstance::new("n", "select_number", Position::default());
    n.inputs.push(PinInstance::new(
        "n_cond",
        Pin::new(
            "n_cond",
            "condition",
            DataType::typed("bool"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        "n_a",
        Pin::new(
            "n_a",
            "a",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        "n_b",
        Pin::new(
            "n_b",
            "b",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        "n_r",
        Pin::new(
            "n_r",
            "result",
            DataType::typed("f64"),
            PinType::Output,
        ),
    ));
    n.properties
        .insert("n_cond".to_string(), serde_json::json!(0.0));
    n.properties
        .insert("n_a".to_string(), serde_json::json!(10.0));
    n.properties
        .insert("n_b".to_string(), serde_json::json!(20.0));
    g.add_node(n);
    g.add_node(assert_eq_float("chk", 20.0, 1e-9));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("n", "n_r", "chk", "chk_a"));
    run(&exec, &g);
}

// ── Statistics ────────────────────────────────────────────────────────────────

#[test]
fn test_mean_4_6_is_5() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("mean", &["a", "b"], &[4.0, 6.0], 5.0, 1e-9),
    );
}
#[test]
fn test_mean_symmetric() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("mean", &["a", "b"], &[3.0, 3.0], 3.0, 1e-9),
    );
}
#[test]
fn test_median_1_3_2_is_2() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("median", &["a", "b", "c"], &[1.0, 3.0, 2.0], 2.0, 1e-9),
    );
}
#[test]
fn test_range_3_to_7_is_4() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("range", &["a", "b"], &[3.0, 7.0], 4.0, 1e-9),
    );
}
#[test]
fn test_range_symmetric() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("range", &["a", "b"], &[7.0, 3.0], 4.0, 1e-9),
    );
}
#[test]
fn test_normalize_midpoint() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "normalize",
            &["value", "min", "max"],
            &[5.0, 0.0, 10.0],
            0.5,
            1e-9,
        ),
    );
}
#[test]
fn test_normalize_at_min() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "normalize",
            &["value", "min", "max"],
            &[0.0, 0.0, 10.0],
            0.0,
            1e-9,
        ),
    );
}
#[test]
fn test_normalize_at_max() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "normalize",
            &["value", "min", "max"],
            &[10.0, 0.0, 10.0],
            1.0,
            1e-9,
        ),
    );
}
#[test]
fn test_percentage_50_of_200() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "percentage",
            &["value", "total"],
            &[100.0, 200.0],
            50.0,
            1e-9,
        ),
    );
}
#[test]
fn test_from_percentage() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "from_percentage",
            &["percentage", "total"],
            &[50.0, 200.0],
            100.0,
            1e-9,
        ),
    );
}
#[test]
fn test_ratio_half() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("ratio", &["a", "b"], &[1.0, 2.0], 0.5, 1e-9),
    );
}
#[test]
fn test_proportion_clamped() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("proportion", &["part", "whole"], &[3.0, 10.0], 0.3, 1e-9),
    );
}

// ── Network / port nodes ──────────────────────────────────────────────────────

#[test]
fn test_http_port_is_80() {
    let (e, _) = exec();
    run(&e, &simple_i64_check("http_port", &[], &[], 80));
}
#[test]
fn test_https_port_is_443() {
    let (e, _) = exec();
    run(&e, &simple_i64_check("https_port", &[], &[], 443));
}
#[test]
fn test_ssh_port_is_22() {
    let (e, _) = exec();
    run(&e, &simple_i64_check("ssh_port", &[], &[], 22));
}
#[test]
fn test_ftp_port_is_21() {
    let (e, _) = exec();
    run(&e, &simple_i64_check("ftp_port", &[], &[], 21));
}
#[test]
fn test_dns_port_is_53() {
    let (e, _) = exec();
    run(&e, &simple_i64_check("dns_port", &[], &[], 53));
}
#[test]
fn test_validate_port_80_is_valid() {
    let (exec, _) = exec();
    run(
        &exec,
        &bool_check("validate_port", &[("port", "i64")], &[80.0], true),
    );
}
#[test]
fn test_validate_port_0_is_invalid() {
    let (exec, _) = exec();
    run(
        &exec,
        &bool_check("validate_port", &[("port", "i64")], &[0.0], false),
    );
}
#[test]
fn test_is_well_known_port_80() {
    let (exec, _) = exec();
    run(
        &exec,
        &bool_check("is_well_known_port", &[("port", "i64")], &[80.0], true),
    );
}
#[test]
fn test_is_dynamic_port_40000() {
    let (exec, _) = exec();
    run(
        &exec,
        &bool_check("is_dynamic_port", &[("port", "i64")], &[50000.0], true),
    );
}

// ── Modulo / division edge cases ──────────────────────────────────────────────

#[test]
fn test_modulo_10_mod_3_is_1() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("modulo", &["a", "b"], &[10.0, 3.0], 1),
    );
}
#[test]
fn test_divide_by_zero_is_zero() {
    let (e, _) = exec();
    run(
        &e,
        &simple_i64_check("divide", &["a", "b"], &[42.0, 0.0], 0),
    );
}

// ── lerp / angle_difference ───────────────────────────────────────────────────

#[test]
fn test_lerp_quarter() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("lerp", &["a", "b", "t"], &[0.0, 100.0, 0.25], 25.0, 1e-9),
    );
}
#[test]
fn test_lerp_three_quarters() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check("lerp", &["a", "b", "t"], &[0.0, 100.0, 0.75], 75.0, 1e-9),
    );
}
#[test]
fn test_angle_difference_zero() {
    let (e, _) = exec();
    run(
        &e,
        &simple_f64_check(
            "angle_difference",
            &["angle1", "angle2"],
            &[1.0, 1.0],
            0.0,
            1e-9,
        ),
    );
}

// ── Color — complex concrete return type (f32, f32, f32, f32) ────────────────
//
// These tests exercise the arena's ability to store and forward 16-byte Copy
// tuples through dispatch.  No generics, no heap allocation — just
// `ptr::write(ret, (f32,f32,f32,f32))` round-tripping through raw bytes.

fn color_node(id: &str, node_type: &str, r: f32, g: f32, b: f32, a: f32) -> NodeInstance {
    let mut n = NodeInstance::new(id, node_type, Position::default());
    for (name, val) in [("r", r), ("g", g), ("b", b), ("a", a)] {
        let pid = format!("{id}_{name}");
        n.inputs.push(PinInstance::new(
            &pid,
            Pin::new(
                &pid,
                name,
                DataType::typed("f32"),
                PinType::Input,
            ),
        ));
        n.properties.insert(pid, serde_json::json!(val as f64));
    }
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("(f32, f32, f32, f32)"),
            PinType::Output,
        ),
    ));
    n
}

fn color_lerp_node(id: &str, t: f32) -> NodeInstance {
    let mut n = NodeInstance::new(id, "color_lerp", Position::default());
    for name in ["a", "b"] {
        let pid = format!("{id}_{name}");
        n.inputs.push(PinInstance::new(
            &pid,
            Pin::new(
                &pid,
                name,
                DataType::typed("(f32, f32, f32, f32)"),
                PinType::Input,
            ),
        ));
    }
    let tp = format!("{id}_t");
    n.inputs.push(PinInstance::new(
        &tp,
        Pin::new(
            &tp,
            "t",
            DataType::typed("f32"),
            PinType::Input,
        ),
    ));
    n.properties.insert(tp, serde_json::json!(t as f64));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("(f32, f32, f32, f32)"),
            PinType::Output,
        ),
    ));
    n
}

fn color_eq_node(id: &str, eps: f32) -> NodeInstance {
    let mut n = NodeInstance::new(id, "color_eq_approx", Position::default());
    for name in ["a", "b"] {
        let pid = format!("{id}_{name}");
        n.inputs.push(PinInstance::new(
            &pid,
            Pin::new(
                &pid,
                name,
                DataType::typed("(f32, f32, f32, f32)"),
                PinType::Input,
            ),
        ));
    }
    let ep = format!("{id}_epsilon");
    n.inputs.push(PinInstance::new(
        &ep,
        Pin::new(
            &ep,
            "epsilon",
            DataType::typed("f32"),
            PinType::Input,
        ),
    ));
    n.properties.insert(ep, serde_json::json!(eps as f64));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("bool"),
            PinType::Output,
        ),
    ));
    n
}

/// color_new with 4 f32 constants → (f32,f32,f32,f32) tuple stored in arena.
/// Routes through color_eq_approx back to assert_true — proves a 16-byte Copy
/// tuple survives a write→read cycle through the byte arena.
#[test]
fn test_color_new_tuple_roundtrip() {
    let (ex, _) = exec();
    let mut g = GraphDescription::new("color_new_roundtrip");
    g.add_node(begin());
    g.add_node(color_node("actual", "color_new", 1.0, 0.5, 0.25, 0.75));
    g.add_node(color_node("expected", "color_new", 1.0, 0.5, 0.25, 0.75));
    g.add_node(color_eq_node("eq", 1e-6));
    g.add_node(assert_true("chk"));
    g.add_connection(d("actual", "actual_r", "eq", "eq_a"));
    g.add_connection(d("expected", "expected_r", "eq", "eq_b"));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("eq", "eq_r", "chk", "chk_c"));
    run(&ex, &g);
}

/// color_lerp(red, blue, t=0.5) == (0.5, 0.0, 0.5, 1.0).
/// Exercises TWO tuple inputs being read from separate arena offsets.
#[test]
fn test_color_lerp_midpoint_red_blue() {
    let (ex, _) = exec();
    let mut g = GraphDescription::new("color_lerp_mid");
    g.add_node(begin());
    g.add_node(color_node("red", "color_new", 1.0, 0.0, 0.0, 1.0));
    g.add_node(color_node("blue", "color_new", 0.0, 0.0, 1.0, 1.0));
    g.add_node(color_lerp_node("mid", 0.5));
    g.add_node(color_node("exp", "color_new", 0.5, 0.0, 0.5, 1.0));
    g.add_node(color_eq_node("eq", 1e-5));
    g.add_node(assert_true("chk"));
    g.add_connection(d("red", "red_r", "mid", "mid_a"));
    g.add_connection(d("blue", "blue_r", "mid", "mid_b"));
    g.add_connection(d("mid", "mid_r", "eq", "eq_a"));
    g.add_connection(d("exp", "exp_r", "eq", "eq_b"));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("eq", "eq_r", "chk", "chk_c"));
    run(&ex, &g);
}

/// color_lerp(a, b, t=0.0) == a  (identity at t=0)
#[test]
fn test_color_lerp_at_zero_is_first_color() {
    let (ex, _) = exec();
    let mut g = GraphDescription::new("color_lerp_t0");
    g.add_node(begin());
    g.add_node(color_node("a", "color_new", 0.8, 0.2, 0.4, 0.9));
    g.add_node(color_node("b", "color_new", 0.0, 1.0, 0.5, 0.1));
    g.add_node(color_lerp_node("res", 0.0));
    g.add_node(color_node("exp", "color_new", 0.8, 0.2, 0.4, 0.9));
    g.add_node(color_eq_node("eq", 1e-5));
    g.add_node(assert_true("chk"));
    g.add_connection(d("a", "a_r", "res", "res_a"));
    g.add_connection(d("b", "b_r", "res", "res_b"));
    g.add_connection(d("res", "res_r", "eq", "eq_a"));
    g.add_connection(d("exp", "exp_r", "eq", "eq_b"));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("eq", "eq_r", "chk", "chk_c"));
    run(&ex, &g);
}

/// color_lerp(a, b, t=1.0) == b  (identity at t=1)
#[test]
fn test_color_lerp_at_one_is_second_color() {
    let (ex, _) = exec();
    let mut g = GraphDescription::new("color_lerp_t1");
    g.add_node(begin());
    g.add_node(color_node("a", "color_new", 0.2, 0.4, 0.6, 0.8));
    g.add_node(color_node("b", "color_new", 0.9, 0.7, 0.3, 0.1));
    g.add_node(color_lerp_node("res", 1.0));
    g.add_node(color_node("exp", "color_new", 0.9, 0.7, 0.3, 0.1));
    g.add_node(color_eq_node("eq", 1e-5));
    g.add_node(assert_true("chk"));
    g.add_connection(d("a", "a_r", "res", "res_a"));
    g.add_connection(d("b", "b_r", "res", "res_b"));
    g.add_connection(d("res", "res_r", "eq", "eq_a"));
    g.add_connection(d("exp", "exp_r", "eq", "eq_b"));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("eq", "eq_r", "chk", "chk_c"));
    run(&ex, &g);
}

// ── Mixed-type Vec — non-Copy heap-allocated complex return types ─────────────
//
// Vec<(i128, i64, bool)>: 24-byte fat pointer on the stack / in the arena,
// heap body with 32-byte elements (i128@align8 + i64 + bool + padding).
//
// Each graph has exactly ONE consumer of each Vec slot — the ptr::read in
// the dispatch shim gives that consumer full ownership; the arena copy goes
// dangling after the consumer drops it, but is never read again.

fn pure_no_input(id: &str, node_type: &str, out_ty: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, node_type, Position::default());
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed(out_ty),
            PinType::Output,
        ),
    ));
    n
}

fn pure_one_input(
    id: &str,
    node_type: &str,
    param: &str,
    in_ty: &str,
    out_ty: &str,
) -> NodeInstance {
    let mut n = NodeInstance::new(id, node_type, Position::default());
    let pid = format!("{id}_{param}");
    n.inputs.push(PinInstance::new(
        &pid,
        Pin::new(
            &pid,
            param,
            DataType::typed(in_ty),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed(out_ty),
            PinType::Output,
        ),
    ));
    n
}

const MIXED_VEC_TY: &str = "Vec < (i128, i64, bool) >";

/// make_mixed_vec() → mixed_vec_check() → assert_true
/// Exercises: heap Vec produced, single ptr::read consumer, bool returned.
#[test]
fn test_mixed_vec_contents_are_correct() {
    let (ex, _) = exec();
    let mut g = GraphDescription::new("mixed_vec_check");
    g.add_node(begin());
    g.add_node(pure_no_input("src", "make_mixed_vec", MIXED_VEC_TY));
    g.add_node(pure_one_input(
        "chk",
        "mixed_vec_check",
        "v",
        MIXED_VEC_TY,
        "bool",
    ));
    g.add_node(assert_true("ok"));
    g.add_connection(d("src", "src_r", "chk", "chk_v"));
    g.add_connection(e("begin", "be", "ok", "ok_e"));
    g.add_connection(d("chk", "chk_r", "ok", "ok_c"));
    run(&ex, &g);
}

/// make_mixed_vec() → mixed_vec_len() → assert_eq_int(3)
/// Exercises: heap allocation round-trips through arena; length is 3.
#[test]
fn test_mixed_vec_length_is_3() {
    let (ex, _) = exec();
    let mut g = GraphDescription::new("mixed_vec_len");
    g.add_node(begin());
    g.add_node(pure_no_input("src", "make_mixed_vec", MIXED_VEC_TY));
    g.add_node(pure_one_input(
        "len",
        "mixed_vec_len",
        "v",
        MIXED_VEC_TY,
        "i64",
    ));
    g.add_node(assert_eq_int("ok", 3));
    g.add_connection(d("src", "src_r", "len", "len_v"));
    g.add_connection(e("begin", "be", "ok", "ok_e"));
    g.add_connection(d("len", "len_r", "ok", "ok_a"));
    run(&ex, &g);
}

/// make_mixed_vec() → mixed_vec_sum()
/// sum = (i128::MAX as i64 wrapping) + (-1) + (1)   [row 0]
///      + 0 + 0 + 0                                  [row 1]
///      + 42 + 999 + 1                               [row 2]
/// = i64::MAX.wrapping_add(-1).wrapping_add(1) + 0 + 1042
/// = i64::MAX.wrapping_add(1).wrapping_add(1042)
/// = i64::MIN.wrapping_add(1042)
/// Exercises: 32-byte struct layout (i128 + i64 + bool + padding) survives
/// the heap→dispatch→sum path intact.
#[test]
fn test_mixed_vec_sum_matches_expected() {
    let expected = {
        let rows: &[(i128, i64, bool)] = &[
            (i128::MAX, -1i64, true),
            (0i128, 0i64, false),
            (42i128, 999i64, true),
        ];
        rows.iter().fold(0i64, |acc, &(a, b, c)| {
            acc.wrapping_add(a as i64)
                .wrapping_add(b)
                .wrapping_add(c as i64)
        })
    };
    let (ex, _) = exec();
    let mut g = GraphDescription::new("mixed_vec_sum");
    g.add_node(begin());
    g.add_node(pure_no_input("src", "make_mixed_vec", MIXED_VEC_TY));
    g.add_node(pure_one_input(
        "sum",
        "mixed_vec_sum",
        "v",
        MIXED_VEC_TY,
        "i64",
    ));
    g.add_node(assert_eq_int("ok", expected));
    g.add_connection(d("src", "src_r", "sum", "sum_v"));
    g.add_connection(e("begin", "be", "ok", "ok_e"));
    g.add_connection(d("sum", "sum_r", "ok", "ok_a"));
    run(&ex, &g);
}
