//! Stateful flow nodes are compiler intrinsics (#874).
//!
//! A flow node with state (`do_once`, `gate`, ..) must keep that state per
//! script instance, and `delay` must never block the game thread. Their old
//! bodies kept state in process-global statics and slept the thread, and the
//! `#[blueprint]` macro registered those bodies as script natives. The nodes
//! are now declarations: the Blueprint compiler lowers them to per-instance
//! variables and `Wait`, and no native exists for another frontend to import
//! by mistake.

use pulsar_script_vm::NativeRegistry;

const INTRINSICS: &[(&str, &[&str])] = &[
    ("gate", &["Then"]),
    ("multi_gate", &["Output0", "Output1", "Output2", "Output3"]),
    ("flip_flop", &["A", "B"]),
    ("do_once", &["Then"]),
    ("do_n", &["Then"]),
    ("delay", &["Completed"]),
    ("retriggerable_delay", &["Completed"]),
];

#[test]
fn stateful_flow_nodes_register_no_script_native() {
    let natives = NativeRegistry::with_engine_natives();
    for (name, _) in INTRINSICS {
        assert!(
            natives.get(&format!("std::{name}")).is_none(),
            "`std::{name}` must not be an importable native"
        );
    }
}

#[test]
fn stateful_flow_nodes_keep_their_palette_metadata() {
    for (name, outputs) in INTRINSICS {
        let node = pulsar_std::get_all_nodes()
            .iter()
            .find(|n| n.name == *name)
            .unwrap_or_else(|| panic!("node `{name}` is still in the palette"));
        assert_eq!(
            node.exec_outputs, *outputs,
            "{name}: exec pins come from the declaration's markers"
        );
    }
}

#[test]
fn the_flow_module_has_no_global_state_or_blocking_calls() {
    let source = include_str!("../src/engine/nodes/flow/mod.rs");
    for forbidden in [
        "static ",
        "thread::sleep",
        "AtomicBool",
        "AtomicI32",
        "Mutex",
    ] {
        // Doc and comment text may mention them; code may not.
        let code: String = source
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains(forbidden),
            "the flow nodes still use `{forbidden}`"
        );
    }
}

#[test]
#[should_panic(expected = "compiler intrinsic")]
fn calling_an_intrinsic_directly_is_an_error_not_shared_state() {
    pulsar_std::do_once(false);
}
