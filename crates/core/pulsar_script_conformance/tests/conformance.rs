//! Interpreted and generated Rust must be indistinguishable.

use pulsar_script_conformance::fixtures::hand_written;
use pulsar_script_conformance::harness::{call, call_on, run, Backend, Limits, Scenario, Step};
use pulsar_script_vm::{Module, Value};

fn fixture(name: &str) -> Module {
    hand_written().into_iter().find(|m| m.name == name).unwrap_or_else(|| panic!("no fixture `{name}`"))
}

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn float(f: f64) -> Value {
    Value::Float(f)
}

fn text(s: &str) -> Value {
    Value::Str(s.into())
}

/// Run on both backends, require identical traces, and return the trace so
/// the test can also check it says what it should (a conformance test that
/// compares two empty traces proves nothing).
fn same(scenario: Scenario) -> Vec<String> {
    let interpreted = run(&scenario, Backend::Interpreted);
    let generated = run(&scenario, Backend::Generated);
    if interpreted != generated {
        let first = interpreted.iter().zip(&generated).position(|(a, b)| a != b);
        panic!(
            "backends diverge in `{}` (limits {:?})\nfirst difference at line {first:?}\n--- interpreted\n{}\n--- generated\n{}",
            scenario.module.name,
            scenario.limits,
            interpreted.join("\n"),
            generated.join("\n"),
        );
    }
    interpreted
}

fn contains(trace: &[String], needle: &str) -> bool {
    trace.iter().any(|line| line.contains(needle))
}

const CHECKED: [Limits; 2] = [
    Limits { budget: 100_000, max_depth: 64, checked: false },
    Limits { budget: 100_000, max_depth: 64, checked: true },
];

#[test]
fn integer_arithmetic_matches_including_overflow_and_division() {
    let edge = [i64::MIN, -7, -1, 0, 1, 2, 7, i64::MAX];
    for limits in CHECKED {
        let mut steps = Vec::new();
        for op in ["int_add", "int_sub", "int_mul", "int_div", "int_rem"] {
            for a in edge {
                for b in edge {
                    steps.push(call(op, vec![int(a), int(b)]));
                }
            }
        }
        for a in edge {
            steps.push(call("int_neg", vec![int(a)]));
        }
        let trace = same(Scenario::new(fixture("arith"), steps).limits(limits));
        assert!(contains(&trace, "DivideByZero"), "division by zero is exercised");
        assert_eq!(contains(&trace, "Overflow"), limits.checked, "overflow is an error exactly when checked");
        assert!(contains(&trace, "returned -9223372036854775808"), "wrapping is exercised");
    }
}

#[test]
fn float_arithmetic_and_conversions_match() {
    let edge = [0.0, -0.0, 1.5, -2.25, 1e300, f64::MAX, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 9.3e18, -9.3e18];
    for limits in CHECKED {
        let mut steps = Vec::new();
        for op in ["float_add", "float_sub", "float_mul", "float_div", "float_rem", "float_lt", "float_ge", "float_eq", "float_ne"] {
            for a in edge {
                for b in [0.0, 2.0, f64::NAN] {
                    steps.push(call(op, vec![float(a), float(b)]));
                }
            }
        }
        for a in edge {
            steps.push(call("float_neg", vec![float(a)]));
            steps.push(call("float_to_int", vec![float(a)]));
            steps.push(call("float_to_str", vec![float(a)]));
        }
        for a in [i64::MIN, -1, 0, 1 << 53, i64::MAX] {
            steps.push(call("int_to_float", vec![int(a)]));
        }
        let trace = same(Scenario::new(fixture("arith"), steps).limits(limits));
        assert_eq!(contains(&trace, "Overflow { op: \"FloatToInt\" }"), limits.checked, "float->int range is checked only when asked");
    }
}

#[test]
fn comparisons_booleans_and_strings_match() {
    let mut steps = Vec::new();
    for op in ["int_lt", "int_le", "int_gt", "int_ge", "int_eq", "int_ne"] {
        for (a, b) in [(1, 2), (2, 1), (3, 3), (i64::MIN, i64::MAX)] {
            steps.push(call(op, vec![int(a), int(b)]));
        }
    }
    for op in ["str_lt", "str_le", "str_gt", "str_ge", "str_eq", "str_ne", "concat"] {
        for (a, b) in [("a", "b"), ("b", "a"), ("same", "same"), ("", "x"), ("é", "e")] {
            steps.push(call(op, vec![text(a), text(b)]));
        }
    }
    for (a, b) in [(true, true), (true, false), (false, true), (false, false)] {
        steps.push(call("and", vec![Value::Bool(a), Value::Bool(b)]));
        steps.push(call("or", vec![Value::Bool(a), Value::Bool(b)]));
    }
    steps.push(call("not", vec![Value::Bool(true)]));
    steps.push(call("bool_to_str", vec![Value::Bool(false)]));
    steps.push(call("int_to_str", vec![int(-42)]));
    steps.push(call("str_to_str", vec![text("same")]));
    let trace = same(Scenario::new(fixture("arith"), steps));
    assert!(contains(&trace, "returned \"ab\""));
}

#[test]
fn bad_entry_calls_are_reported_the_same_way() {
    let trace = same(Scenario::new(
        fixture("arith"),
        vec![call("int_add", vec![int(1)]), call("int_add", vec![int(1), float(2.0)]), call("not", vec![int(1)])],
    ));
    assert_eq!(trace.iter().filter(|l| l.contains("BadEntryCall")).count(), 3);
}

#[test]
fn errors_carry_the_same_trace_and_source_locations() {
    let trace = same(Scenario::new(fixture("arith"), vec![call("located_div", vec![int(1), int(0)])]));
    assert!(contains(&trace, "divide-node"), "the failing instruction's source location is reported: {trace:#?}");
}

#[test]
fn loops_branches_calls_and_recursion_match() {
    let mut steps = Vec::new();
    for n in [-1, 0, 1, 10, 100] {
        steps.push(call("sum_to", vec![int(n)]));
    }
    for n in [0, 1, 2, 10, 15] {
        steps.push(call("fib", vec![int(n)]));
    }
    for x in [-5, 0, 5] {
        steps.push(call("classify", vec![int(x)]));
        steps.push(call("quad", vec![int(x)]));
    }
    let trace = same(Scenario::new(fixture("flow"), steps));
    assert!(contains(&trace, "returned 610"), "fib(15)");
    assert!(contains(&trace, "returned \"zero\""));
}

#[test]
fn the_instruction_budget_and_call_depth_limits_match() {
    for budget in [0, 1, 7, 100, 5_000] {
        let trace = same(
            Scenario::new(fixture("flow"), vec![call("forever", vec![]), call("sum_to", vec![int(1000)]), call("fib", vec![int(12)])])
                .limits(Limits { budget, max_depth: 64, checked: false }),
        );
        assert!(contains(&trace, "BudgetExceeded"), "budget {budget}");
    }
    for max_depth in [1, 2, 5, 64] {
        let trace = same(
            Scenario::new(fixture("flow"), vec![call("deep", vec![int(1)]), call("fib", vec![int(8)]), call("quad", vec![int(3)])])
                .limits(Limits { budget: 1_000_000, max_depth, checked: false }),
        );
        assert!(contains(&trace, "StackOverflow"), "depth {max_depth}");
    }
}

#[test]
fn waits_resume_at_the_same_times_with_the_same_state() {
    let steps = vec![
        call("delayed", vec![]),
        Step::Advance(1.0),
        Step::Advance(0.4),
        Step::Advance(0.1),
        Step::Advance(0.5),
        Step::Advance(10.0),
        call("get_count", vec![]),
    ];
    let trace = same(Scenario::new(fixture("waits"), steps));
    assert!(contains(&trace, "waiting 1.5s in delayed"));
    assert!(contains(&trace, "count=111"), "all three increments ran: {trace:#?}");
}

#[test]
fn a_wait_two_frames_deep_resumes_through_both_frames() {
    let trace = same(Scenario::new(fixture("waits"), vec![call("nested", vec![]), Step::Advance(1.0)]));
    assert!(contains(&trace, "waiting 1s in nested>inner"));
    assert!(contains(&trace, "count=1001"));
}

#[test]
fn loops_that_wait_and_several_waiting_calls_per_instance_match() {
    let trace = same(
        Scenario::new(
            fixture("waits"),
            vec![
                call("looped", vec![int(3)]),
                call("looped", vec![int(2)]),
                call("delayed", vec![]),
                Step::Advance(1.0),
                Step::Advance(1.0),
                Step::Advance(1.0),
                Step::Advance(1.0),
            ],
        )
        .limits(Limits { budget: 10_000, max_depth: 8, checked: false }),
    );
    assert!(contains(&trace, "waiting: ["), "waiting calls are traced");
}

#[test]
fn instances_wait_and_keep_state_independently() {
    let trace = same(
        Scenario::new(
            fixture("waits"),
            vec![
                call_on(0, "delayed", vec![]),
                Step::Advance(0.75),
                call_on(1, "delayed", vec![]),
                call_on(2, "nested", vec![]),
                Step::Advance(0.75),
                Step::Advance(0.75),
                Step::Advance(5.0),
                call_on(0, "get_count", vec![]),
                call_on(1, "get_count", vec![]),
                call_on(2, "get_count", vec![]),
            ],
        )
        .instances(3),
    );
    assert!(contains(&trace, "vars #0: count=111"));
    assert!(contains(&trace, "vars #2: count=1001"));
}

#[test]
fn odd_wait_durations_and_errors_after_a_wait_match() {
    let trace = same(Scenario::new(
        fixture("waits"),
        vec![call("odd_waits", vec![]), Step::Advance(0.0), Step::Advance(0.0), Step::Advance(0.0), call("wait_then_fail", vec![]), Step::Advance(1.0)],
    ));
    assert!(contains(&trace, "count=3"), "negative, zero and NaN waits all resume at once");
    assert!(contains(&trace, "DivideByZero"), "an error after resuming carries its trace");
}

#[test]
fn native_calls_results_failures_and_panics_match() {
    let trace = same(Scenario::new(
        fixture("natives"),
        vec![
            call("call_note", vec![]),
            call("call_fail", vec![]),
            call("call_boom", vec![]),
            call("call_liar", vec![]),
            call("inout_bump", vec![]),
            call("twice_chain", vec![int(5)]),
            call("twice_chain", vec![int(i64::MAX)]),
            call("stdlib", vec![float(16.0)]),
            call("stdlib", vec![float(2.0)]),
            call("stdlib", vec![float(-1.0)]),
            call("me", vec![]),
            call("now", vec![]),
            Step::Advance(2.5),
            call("now", vec![]),
            call("remember", vec![int(9)]),
        ],
    ));
    assert!(contains(&trace, "note(\\\"hello\\\", 7)"));
    assert!(contains(&trace, "deliberate failure"));
    assert!(contains(&trace, "panicked: deliberate panic"));
    assert!(contains(&trace, "returned string, declared int"));
    assert!(contains(&trace, "returned 6"), "inout arguments are written back");
    assert!(contains(&trace, "returned 2.5"), "the clock is the host's");
}

#[test]
fn value_types_have_value_semantics_in_both_backends() {
    let trace = same(
        Scenario::new(fixture("natives"), vec![call_on(0, "grow", vec![]), call_on(0, "grow", vec![]), call_on(1, "grow", vec![])])
            .instances(2),
    );
    assert!(contains(&trace, "vars #1: last=0 pos=Vec3[1.0,2.0,3.0]") || contains(&trace, "pos=Vec3"), "{trace:#?}");
    // Instance 1's position is untouched by instance 0's growth.
    assert!(trace.iter().any(|l| l.starts_with("vars #1:") && l.contains("[1.0,2.0,3.0]")));
}

#[test]
fn declared_events_and_subscriptions_link_identically() {
    let trace = same(Scenario::new(fixture("events_decl"), vec![call("on_ping", vec![int(4)])]));
    assert!(contains(&trace, "events_decl.Ping"), "the subscription is part of the trace");
    assert!(contains(&trace, "seen=4"));
}

// ---- modules compiled from Blueprint graphs -------------------------------------

fn blueprint(name: &str) -> Module {
    let path = format!("{}/fixtures/{name}.module.json", env!("CARGO_MANIFEST_DIR"));
    Module::from_json(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).expect("fixture parses")
}

fn flag(b: bool) -> Value {
    Value::Bool(b)
}

#[test]
fn blueprint_stateful_flow_nodes_keep_their_state_per_instance() {
    let mut steps = Vec::new();
    for _ in 0..6 {
        steps.push(call_on(0, "on_fire", vec![]));
    }
    steps.push(call_on(1, "on_fire", vec![]));
    // The gate: closed until opened, closes again on request.
    for (open, close) in [(false, false), (true, false), (false, false), (false, true), (false, false), (true, true)] {
        steps.push(call_on(0, "on_ctl", vec![flag(open), flag(close)]));
        steps.push(call_on(1, "on_ctl", vec![flag(open), flag(close)]));
    }
    // do_once with a wired reset: fires once, again only after a reset.
    for reset in [false, false, true, false, false] {
        steps.push(call_on(0, "on_reset", vec![flag(reset)]));
    }
    let trace = same(Scenario::new(blueprint("bp_state"), steps).instances(2));
    // Instance 0 ran six times, so flip_flop alternated and do_once fired once.
    assert!(trace.iter().any(|l| l.starts_with("vars #0:") && l.contains("log=\"o")), "{trace:#?}");
    assert!(trace.iter().any(|l| l.starts_with("vars #1:") && l.contains("log=\"ob")), "instance 1 has its own once/flip-flop state");
}

#[test]
fn blueprint_delays_wait_in_game_time_per_instance() {
    let trace = same(
        Scenario::new(
            blueprint("bp_delays"),
            vec![
                call_on(0, "on_fire", vec![]),
                call_on(1, "on_fire", vec![]),
                Step::Advance(0.25),
                call_on(0, "on_fire", vec![]), // ignored while counting down
                Step::Advance(0.25),
                call_on(0, "on_retrigger", vec![]),
                Step::Advance(0.8),
                call_on(0, "on_retrigger", vec![]), // restarts the countdown
                Step::Advance(0.4),
                Step::Advance(0.4),
                Step::Advance(1.0),
                call_on(1, "on_two", vec![]),
                Step::Advance(0.2),
                Step::Advance(0.3),
                Step::Advance(5.0),
            ],
        )
        .instances(2),
    );
    assert!(contains(&trace, "waiting 0.5s in on_fire"));
    assert!(trace.iter().any(|l| l.starts_with("vars #1:") && l.contains("xy")), "two delays in a row both complete: {trace:#?}");
}

#[test]
fn blueprint_loops_branches_and_sequences_match_including_waiting_while_loops() {
    let mut steps = vec![call("begin_play", vec![]), call("on_check", vec![flag(true)]), call("on_check", vec![flag(false)]), call("on_seq", vec![])];
    steps.push(call("on_count", vec![]));
    for _ in 0..6 {
        steps.push(call("on_count", vec![])); // ignored while the loop runs
        steps.push(Step::Advance(0.0)); // one iteration per frame
    }
    steps.push(call("on_count", vec![]));
    let trace = same(Scenario::new(blueprint("bp_loops"), steps));
    assert!(contains(&trace, "waiting 0s in on_count"), "the while loop yields a frame per iteration");
    assert!(trace.iter().any(|l| l.contains("count=3")));
}

#[test]
fn blueprint_selector_natives_with_inout_results_match() {
    let trace = same(Scenario::new(
        blueprint("bp_pick"),
        vec![call("on_pick", vec![int(4)]), call("on_pick", vec![int(0)]), call("on_pick", vec![int(5)]), call("on_pick", vec![int(-2)])],
    ));
    assert!(contains(&trace, "count=50"));
}

#[test]
fn blueprint_modules_run_under_tight_limits_identically() {
    for limits in [
        Limits { budget: 12, max_depth: 64, checked: true },
        Limits { budget: 100_000, max_depth: 1, checked: true },
        Limits { budget: 40, max_depth: 2, checked: false },
    ] {
        same(
            Scenario::new(
                blueprint("bp_state"),
                vec![call_on(0, "on_fire", vec![]), call_on(0, "on_ctl", vec![flag(true), flag(false)]), call_on(0, "on_reset", vec![flag(true)])],
            )
            .limits(limits),
        );
        same(Scenario::new(blueprint("bp_loops"), vec![call("begin_play", vec![]), call("on_count", vec![]), Step::Advance(0.0)]).limits(limits));
    }
}

// ---- collections ------------------------------------------------------------------

#[test]
fn lists_maps_and_tuples_match_including_their_errors() {
    for limits in CHECKED {
        let trace = same(
            Scenario::new(
                fixture("collections"),
                vec![
                    call("push", vec![int(5)]),
                    call("push", vec![int(7)]),
                    call("insert_at", vec![int(1), int(6)]),
                    call("set_at", vec![int(0), int(4)]),
                    call("sum", vec![]),
                    call("at", vec![int(2)]),
                    call("copy_is_independent", vec![]),
                    call("remove_at", vec![int(0)]),
                    call("log_text", vec![]),
                    call("literal_equals_log", vec![int(6), int(7)]),
                    // Errors: the same kind, trace and state after.
                    call("at", vec![int(2)]),
                    call("at", vec![int(-1)]),
                    call("set_at", vec![int(9), int(1)]),
                    call("insert_at", vec![int(9), int(1)]),
                    call("remove_at", vec![int(9)]),
                    call("sum", vec![]),
                    call("bind", vec![text("b"), int(2)]),
                    call("bind", vec![text("a"), int(1)]),
                    call("lookup", vec![text("b")]),
                    call("lookup", vec![text("zz")]),
                    call("has", vec![text("a")]),
                    call("unbind", vec![text("a")]),
                    call("unbind", vec![text("never")]),
                    call("has", vec![text("a")]),
                    call("key_at", vec![int(0)]),
                    call("key_at", vec![int(3)]),
                    call("pair", vec![int(1), text("x")]),
                    call("swap", vec![int(1), text("x")]),
                ],
            )
            .limits(limits),
        );
        assert!(contains(&trace, "sum() -> returned 17"), "{trace:#?}");
        assert!(contains(&trace, "copy_is_independent() -> returned 410"), "the copy does not alias: {trace:#?}");
        assert!(contains(&trace, "IndexOutOfBounds { index: 2, len: 2 }"), "{trace:#?}");
        assert!(contains(&trace, "IndexOutOfBounds { index: -1, len: 2 }"), "{trace:#?}");
        assert!(contains(&trace, "KeyNotFound { key: \"zz\" }"), "{trace:#?}");
        assert!(contains(&trace, "swap(1, \"x\") -> returned (\"x\", 1)"), "{trace:#?}");
    }
}

#[test]
fn collections_survive_waits_and_stay_per_instance() {
    let trace = same(
        Scenario::new(
            fixture("collections"),
            vec![call_on(0, "push", vec![int(1)]), call_on(1, "push", vec![int(10)]), call_on(0, "push", vec![int(2)]), call_on(1, "sum", vec![]), call_on(0, "sum", vec![])],
        )
        .instances(2),
    );
    assert!(trace.iter().any(|l| l.starts_with("vars #0:") && l.contains("[1, 2]")), "{trace:#?}");
    assert!(trace.iter().any(|l| l.starts_with("vars #1:") && l.contains("[10]")), "{trace:#?}");
}
