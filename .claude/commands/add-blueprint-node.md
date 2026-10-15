# Add a Blueprint Node

How to add a new callable node to the blueprint graph system (e.g. a new debug print, math op, game function).

## Where nodes live

Standard library nodes: `Pulsar-Native/crates/core/pulsar_std/src/engine/nodes/`
Each category has its own directory (`debug/`, `math/`, `string/`, etc.).

The `#[blueprint]` attribute registers each node in `pulsar_std`'s compile-time
registry (`pulsar_std::get_all_nodes()`). The Blueprint editor builds its
palette and graph checks from that registry (`plugins/vendor/blueprint_editor/src/std_nodes.rs`),
so there is no separate metadata to write.

---

## Step 1 — Implement the function in pulsar_std

```
crates/core/pulsar_std/src/engine/nodes/<category>/mod.rs
```

```rust
/// # My New Node
/// Short doc comment shown in the editor tooltip.
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn my_new_node(arg1: f64, arg2: String) -> bool {
    // implementation
    true
}
```

- `type`: `NodeTypes::pure` (no exec pins), `NodeTypes::fn_` (exec in/out),
  `NodeTypes::control_flow` or `NodeTypes::event`.
- Parameters become input pins; the return value becomes the output pin
  (`()` means no data output).
- Simple scalar types: `f64`, `bool`, `String`, `i64`.

## Step 2 — Control-flow nodes

Declare each exec output with `exec_output!("Label")` in the body:

```rust
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn branch(condition: bool) {
    if condition {
        exec_output!("True");
    } else {
        exec_output!("False");
    }
}
```

Nodes the Blueprint compiler lowers itself (e.g. `delay`, `do_once`) also take
`intrinsic: true`.

---

## Step 3 — Verify

1. `cargo check -p pulsar_std`
2. `cargo test -p blueprint_editor_plugin --test std_nodes` (the node is in the editor's catalogue)

## Checklist

- [ ] `#[blueprint(...)]` on the function, in a module `pulsar_std` already includes
- [ ] `cargo check -p pulsar_std` passes
- [ ] The node shows up in `pulsar_std::get_all_nodes()` with the pins you expect
