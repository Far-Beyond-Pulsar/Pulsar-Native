//! How much of pulsar_std scripts can call, and an explicit list of what
//! they cannot and why.
//!
//! Every data node (`pure` or `fn_`) is a script native, a generic native
//! (the `array_*` nodes, instantiated per element type), or on
//! [`NOT_SCRIPTABLE`]. A new node that is none of these fails the test: give
//! it a representable signature, or add it to the list with the reason.
//! `cargo test -p pulsar_std --test native_coverage -- --nocapture` prints
//! the report CI shows.

use pulsar_script_vm::NativeRegistry;
use pulsar_std::{get_all_nodes, NodeTypes};

/// Nodes that cannot be script natives, with the reason. All of them hold
/// something that is meaningful only inside one process (a thread, a lock,
/// an atomic, a channel) or end the process.
const NOT_SCRIPTABLE: &[(&str, &str)] = &[
    ("create_mutex", "a lock handle"),
    ("lock_mutex", "a lock handle"),
    ("unlock_mutex", "a lock handle"),
    ("spawn_thread", "a thread handle and a closure"),
    ("join_thread", "a thread handle"),
    ("channel_new", "channel endpoints"),
    ("channel_send", "channel endpoints"),
    ("channel_recv", "channel endpoints"),
    ("atomic_i32_new", "shared mutable state behind a handle"),
    ("atomic_i32_add", "shared mutable state behind a handle"),
    ("atomic_i32_load", "shared mutable state behind a handle"),
    ("atomic_i32_store", "shared mutable state behind a handle"),
    ("atomic_bool_new", "shared mutable state behind a handle"),
    ("atomic_bool_load", "shared mutable state behind a handle"),
    ("atomic_bool_store", "shared mutable state behind a handle"),
    ("process_exit", "ends the engine process"),
    ("process_abort", "ends the engine process"),
];

fn data_nodes() -> impl Iterator<Item = &'static pulsar_std::NodeMetadata> {
    get_all_nodes().iter().filter(|node| matches!(node.node_type, NodeTypes::pure | NodeTypes::fn_))
}

#[test]
fn every_data_node_is_a_native_or_explained() {
    let registry = NativeRegistry::with_engine_natives();
    let name = |node: &pulsar_std::NodeMetadata| format!("std::{}", node.name);
    let (mut natives, mut generics, mut excluded) = (0, 0, 0);
    let mut unexplained = Vec::new();
    for node in data_nodes() {
        if registry.get(&name(node)).is_some() {
            natives += 1;
        } else if registry.generic(&name(node)).is_some() {
            generics += 1;
        } else if NOT_SCRIPTABLE.iter().any(|(excluded, _)| *excluded == node.name) {
            excluded += 1;
        } else {
            let params: Vec<_> = node.params.iter().map(|p| format!("{}: {}", p.name, p.ty)).collect();
            unexplained.push(format!("{}({}) -> {}", node.name, params.join(", "), node.return_type.unwrap_or("()")));
        }
    }
    let total = natives + generics + excluded + unexplained.len();
    eprintln!(
        "script coverage of pulsar_std data nodes: {} of {total} callable ({natives} natives, {generics} generic), \
         {excluded} not scriptable by design, {} unaccounted for",
        natives + generics,
        unexplained.len()
    );
    assert!(unexplained.is_empty(), "nodes that are neither script natives nor listed in NOT_SCRIPTABLE:\n  {}", unexplained.join("\n  "));
}

#[test]
fn the_exclusion_list_names_real_nodes_that_really_are_not_natives() {
    let registry = NativeRegistry::with_engine_natives();
    for (name, reason) in NOT_SCRIPTABLE {
        assert!(data_nodes().any(|node| node.name == *name), "`{name}` ({reason}) is not a node: remove it from the list");
        assert!(
            registry.get(&format!("std::{name}")).is_none() && registry.generic(&format!("std::{name}")).is_none(),
            "`{name}` is a native now: remove it from the list"
        );
    }
}
