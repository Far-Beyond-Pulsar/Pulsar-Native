/// Tests for generic blueprint nodes (array operations).
/// Verifies that type-erased size-dispatch shims work end-to-end.
use graphy::{
    Connection, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position,
};
use pbgc::compile_graph_to_bytecode;
use pulsar_bp_executor::BpExecutor;
use pulsar_std_bundle::{expected_sha256, extract_to_tempfile};

fn exec() -> (BpExecutor, pulsar_std_bundle::TempLib) {
    let tmp = extract_to_tempfile().unwrap();
    let e = BpExecutor::load(&tmp.path, Some(expected_sha256())).unwrap();
    (e, tmp)
}

fn run(e: &BpExecutor, g: &GraphDescription) {
    let mut progs = compile_graph_to_bytecode(g).expect("compile");
    for p in &mut progs {
        e.prepare(p).unwrap_or_else(|err| panic!("prepare: {err}"));
        pbgc::vm::run(p).unwrap_or_else(|err| panic!("run: {err}"));
    }
}

fn e(f: &str, fp: &str, t: &str, tp: &str) -> Connection {
    Connection::new(f, fp, t, tp, ConnectionType::Execution)
}
fn d(f: &str, fp: &str, t: &str, tp: &str) -> Connection {
    Connection::new(f, fp, t, tp, ConnectionType::Data)
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

fn add_node(id: &str, a: i64, b: i64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "add", Position::default());
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
    n.properties
        .insert(format!("{id}_a"), serde_json::json!(a as f64));
    n.properties
        .insert(format!("{id}_b"), serde_json::json!(b as f64));
    n
}

fn array_new_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "array_new", Position::default());
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("Vec"),
            PinType::Output,
        ),
    ));
    n
}

fn array_push_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "array_push", Position::default());
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
        &format!("{id}_arr"),
        Pin::new(
            &format!("{id}_arr"),
            "array",
            DataType::typed("Vec"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_itm"),
        Pin::new(
            &format!("{id}_itm"),
            "item",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_eo"),
        Pin::new(
            &format!("{id}_eo"),
            "exec",
            DataType::Exec,
            PinType::Output,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("Vec"),
            PinType::Output,
        ),
    ));
    n
}

/// array_push where the item T is itself a Vec (nested vec test, size-dispatch arm = 24).
fn array_push_vec_item_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "array_push", Position::default());
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
        &format!("{id}_arr"),
        Pin::new(
            &format!("{id}_arr"),
            "array",
            DataType::typed("Vec"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_itm"),
        Pin::new(
            &format!("{id}_itm"),
            "item",
            DataType::typed("Vec"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_eo"),
        Pin::new(
            &format!("{id}_eo"),
            "exec",
            DataType::Exec,
            PinType::Output,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("Vec"),
            PinType::Output,
        ),
    ));
    n
}

fn array_clear_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "array_clear", Position::default());
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
        &format!("{id}_arr"),
        Pin::new(
            &format!("{id}_arr"),
            "array",
            DataType::typed("Vec"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_eo"),
        Pin::new(
            &format!("{id}_eo"),
            "exec",
            DataType::Exec,
            PinType::Output,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("Vec"),
            PinType::Output,
        ),
    ));
    n
}

fn array_set_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "array_set", Position::default());
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
        &format!("{id}_arr"),
        Pin::new(
            &format!("{id}_arr"),
            "array",
            DataType::typed("Vec"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_idx"),
        Pin::new(
            &format!("{id}_idx"),
            "index",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_val"),
        Pin::new(
            &format!("{id}_val"),
            "value",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_eo"),
        Pin::new(
            &format!("{id}_eo"),
            "exec",
            DataType::Exec,
            PinType::Output,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("Vec"),
            PinType::Output,
        ),
    ));
    n
}

fn array_length_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "array_length", Position::default());
    n.inputs.push(PinInstance::new(
        &format!("{id}_arr"),
        Pin::new(
            &format!("{id}_arr"),
            "array",
            DataType::typed("Vec"),
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
    n
}

fn array_is_empty_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "array_is_empty", Position::default());
    n.inputs.push(PinInstance::new(
        &format!("{id}_arr"),
        Pin::new(
            &format!("{id}_arr"),
            "array",
            DataType::typed("Vec"),
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

// ── Pure generic node tests ───────────────────────────────────────────────────
// These exercise the T=() shim path (no bare T params → TypeSlots not needed).

#[test]
fn test_array_new_length_is_zero() {
    // array_new() → array_length → assert 0
    let (ex, _) = exec();
    let mut g = GraphDescription::new("array_new_len");
    g.add_node(begin());
    g.add_node(array_new_node("new"));
    g.add_node(array_length_node("len"));
    g.add_node(assert_eq_int("chk", 0));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("new", "new_r", "len", "len_arr"));
    g.add_connection(d("len", "len_r", "chk", "chk_a"));
    run(&ex, &g);
}

#[test]
fn test_array_new_is_empty() {
    // array_new() → array_is_empty → assert true
    let (ex, _) = exec();
    let mut g = GraphDescription::new("array_new_empty");
    g.add_node(begin());
    g.add_node(array_new_node("new"));
    g.add_node(array_is_empty_node("mt"));
    g.add_node(assert_true("chk"));
    g.add_connection(e("begin", "be", "chk", "chk_e"));
    g.add_connection(d("new", "new_r", "mt", "mt_arr"));
    g.add_connection(d("mt", "mt_r", "chk", "chk_c"));
    run(&ex, &g);
}

// ── fn_ generic node tests ────────────────────────────────────────────────────
// These exercise the size-dispatch shim path (bare T param → TypeSlot needed).

#[test]
fn test_array_push_i64_no_panic() {
    // add(0,42) provides an i64 (8 bytes) as T; TypeSlot[0]={size:8,align:8}
    // begin → array_push(array_new, add_result) → assert_eq_int(pre_push_len, 0)
    let (ex, _) = exec();
    let mut g = GraphDescription::new("array_push_i64");
    g.add_node(begin());
    g.add_node(array_new_node("new"));
    g.add_node(add_node("item", 0, 42));
    g.add_node(array_length_node("pre_len"));
    g.add_node(array_push_node("push"));
    g.add_node(assert_eq_int("chk", 0));
    // pure preamble: new → pre_len (both pure, no fn_ dependency)
    g.add_connection(d("new", "new_r", "pre_len", "pre_len_arr"));
    // exec chain: begin → push → chk
    g.add_connection(e("begin", "be", "push", "push_e"));
    g.add_connection(e("push", "push_eo", "chk", "chk_e"));
    // data into push (both pure sources — valid)
    g.add_connection(d("new", "new_r", "push", "push_arr"));
    g.add_connection(d("item", "item_r", "push", "push_itm"));
    // pre-push length into assert
    g.add_connection(d("pre_len", "pre_len_r", "chk", "chk_a"));
    run(&ex, &g);
}

#[test]
fn test_array_push_twice_no_panic() {
    // Push twice with different values; verify no crash.
    let (ex, _) = exec();
    let mut g = GraphDescription::new("array_push_twice");
    g.add_node(begin());
    g.add_node(array_new_node("new"));
    g.add_node(add_node("item1", 0, 10));
    g.add_node(add_node("item2", 0, 20));
    g.add_node(array_push_node("push1"));
    g.add_node(array_push_node("push2"));
    // exec: begin → push1 → push2
    g.add_connection(e("begin", "be", "push1", "push1_e"));
    g.add_connection(e("push1", "push1_eo", "push2", "push2_e"));
    // push1 reads from array_new and item1 (pure sources)
    g.add_connection(d("new", "new_r", "push1", "push1_arr"));
    g.add_connection(d("item1", "item1_r", "push1", "push1_itm"));
    // push2 reads from push1's result (fn_ output) and item2 (pure)
    g.add_connection(d("push1", "push1_r", "push2", "push2_arr"));
    g.add_connection(d("item2", "item2_r", "push2", "push2_itm"));
    run(&ex, &g);
}

#[test]
fn test_array_push_then_clear_no_panic() {
    // begin → push → clear; verifies both fn_ generic shims can execute in sequence.
    let (ex, _) = exec();
    let mut g = GraphDescription::new("array_push_clear");
    g.add_node(begin());
    g.add_node(array_new_node("new"));
    g.add_node(add_node("item", 0, 99));
    g.add_node(array_push_node("push"));
    g.add_node(array_clear_node("clr"));
    g.add_connection(e("begin", "be", "push", "push_e"));
    g.add_connection(e("push", "push_eo", "clr", "clr_e"));
    g.add_connection(d("new", "new_r", "push", "push_arr"));
    g.add_connection(d("item", "item_r", "push", "push_itm"));
    g.add_connection(d("push", "push_r", "clr", "clr_arr"));
    run(&ex, &g);
}

#[test]
fn test_array_set_no_panic() {
    // Push an element then set it; both use bare T dispatch.
    let (ex, _) = exec();
    let mut g = GraphDescription::new("array_set");
    g.add_node(begin());
    g.add_node(array_new_node("new"));
    g.add_node(add_node("item", 0, 7));
    g.add_node(add_node("value", 0, 42));
    g.add_node(add_node("idx", 0, 0));
    g.add_node(array_push_node("push"));
    g.add_node(array_set_node("set"));
    g.add_connection(e("begin", "be", "push", "push_e"));
    g.add_connection(e("push", "push_eo", "set", "set_e"));
    g.add_connection(d("new", "new_r", "push", "push_arr"));
    g.add_connection(d("item", "item_r", "push", "push_itm"));
    g.add_connection(d("push", "push_r", "set", "set_arr"));
    g.add_connection(d("idx", "idx_r", "set", "set_idx"));
    g.add_connection(d("value", "value_r", "set", "set_val"));
    run(&ex, &g);
}

#[test]
fn test_array_push_uses_length_as_item() {
    // array_length(array_new) = 0 (pure), used as the item T for push.
    // This verifies the TypeSlot comes from array_length's return type (i64, size=8).
    let (ex, _) = exec();
    let mut g = GraphDescription::new("array_push_len_as_item");
    g.add_node(begin());
    g.add_node(array_new_node("new"));
    g.add_node(array_length_node("len")); // pure: len(new) = 0
    g.add_node(array_push_node("push"));
    g.add_node(assert_eq_int("chk", 0));
    // pure: len reads from new
    g.add_connection(d("new", "new_r", "len", "len_arr"));
    // exec: begin → push → chk
    g.add_connection(e("begin", "be", "push", "push_e"));
    g.add_connection(e("push", "push_eo", "chk", "chk_e"));
    // push reads vec from new, item from len (both pure — TypeSlot from len's return)
    g.add_connection(d("new", "new_r", "push", "push_arr"));
    g.add_connection(d("len", "len_r", "push", "push_itm"));
    // assert that pre-push length was 0 (verifying pure chain ran correctly)
    g.add_connection(d("len", "len_r", "chk", "chk_a"));
    run(&ex, &g);
}

#[test]
fn test_nested_vec_push_no_panic() {
    // Vec<Vec<T>>: push an inner Vec into an outer Vec.
    // item T = Vec<()> = 24 bytes on 64-bit → size-dispatch arm "[u8; 24]".
    let (ex, _) = exec();
    let mut g = GraphDescription::new("nested_vec_push");
    g.add_node(begin());
    g.add_node(array_new_node("outer")); // Vec<Vec<?>>
    g.add_node(array_new_node("inner")); // Vec<?> — this becomes the item
    g.add_node(array_push_vec_item_node("push")); // outer.push(inner)
                                                  // exec: begin → push
    g.add_connection(e("begin", "be", "push", "push_e"));
    // data: outer into push's array slot, inner into push's item slot
    g.add_connection(d("outer", "outer_r", "push", "push_arr"));
    g.add_connection(d("inner", "inner_r", "push", "push_itm"));
    run(&ex, &g);
}
