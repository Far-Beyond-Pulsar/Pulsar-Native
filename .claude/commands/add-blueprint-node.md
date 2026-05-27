# Add a Blueprint Node

How to add a new callable node to the blueprint graph system (e.g. a new debug print, math op, game function).

## Where nodes live

Standard library nodes: `Pulsar-Native/crates/pulsar_std/src/engine/nodes/`
Each category has its own directory (`debug/`, `math/`, `string/`, etc.).

Node metadata (what the editor shows): `PBGC/src/metadata.rs` — `BlueprintMetadataProvider`

---

## Step 1 — Implement the function in pulsar_std

```
crates/pulsar_std/src/engine/nodes/<category>/mod.rs
```

```rust
/// Short doc comment shown in the editor tooltip.
pub fn my_new_node(arg1: f64, arg2: String) -> bool {
    // implementation
    true
}
```

Rules:
- All types must be `Send + Sync + 'static` (no borrows, no Rc)
- Simple scalar types: `f64`, `bool`, `String`, `i64`
- Return type becomes the output data pin; `()` means exec-only output
- The function is called from blueprint bytecode; keep it infallible

Re-export from `pulsar_std/src/lib.rs` if not already glob-exported.

---

## Step 2 — Register node metadata in PBGC

Open `PBGC/src/metadata.rs`.

Find `impl BlueprintMetadataProvider` and add an entry in the appropriate list:

```rust
NodeMetadata {
    node_type: "my_new_node".into(),      // matches fn name exactly
    display_name: "My New Node".into(),
    category: "Category/Subcategory".into(),
    node_type_enum: NodeTypes::pure,      // pure = no exec pins, fn = has exec pins
    inputs: vec![
        PinMetadata { id: "exec".into(), name: "".into(), data_type: DataType::Execution },
        PinMetadata { id: "arg1".into(), name: "Arg 1".into(), data_type: DataType::Data { base_type: "f64".into(), ..Default::default() } },
        PinMetadata { id: "arg2".into(), name: "Arg 2".into(), data_type: DataType::Data { base_type: "String".into(), ..Default::default() } },
    ],
    outputs: vec![
        PinMetadata { id: "exec".into(), name: "".into(), data_type: DataType::Execution },
        PinMetadata { id: "return_value".into(), name: "Return Value".into(), data_type: DataType::Data { base_type: "bool".into(), ..Default::default() } },
    ],
    ..Default::default()
},
```

NodeTypes variants: `event`, `pure`, `fn_`, `control_flow`

---

## Step 3 — Handle codegen in BlueprintCodeGenerator (if non-trivial)

`PBGC/src/codegen/rust_codegen.rs`

Most nodes are handled automatically by `generate_node_call()` which emits:
```rust
let node_<uuid>_result = my_new_node(arg1, arg2);
```

If your node needs special codegen (e.g. a control-flow node with `exec_output!`), override in `generate_special_node()`.

---

## Step 4 — Add a pulsar_std function source template (control-flow nodes only)

If the node is a control-flow node (Branch, Sequence, etc.), add its function source in `PBGC/src/metadata.rs` under `function_source`:

```rust
function_source: Some(r#"
    fn my_node(condition: bool) {
        if condition {
            exec_output!("True");
        } else {
            exec_output!("False");
        }
    }
"#.into()),
```

The AST transformer (`Graphy/src/utils/ast_transform.rs`) replaces each `exec_output!("Label")` with the compiled code of the connected chain.

---

## Step 5 — Push and cascade revs

1. `cargo check -p pulsar_std` in Pulsar-Native — verify no compile errors
2. `cargo check -p pbgc` in PBGC — verify metadata compiles
3. Push PBGC, then cascade via `/bump-deps`

---

## Checklist

- [ ] Function in `pulsar_std` compiles (`cargo check -p pulsar_std`)
- [ ] Exported from `pulsar_std/src/lib.rs` (or parent module)
- [ ] `NodeMetadata` entry in `PBGC/src/metadata.rs`
- [ ] Input/output pins match function signature exactly
- [ ] `cargo check -p pbgc` passes
- [ ] Rev cascade done (PBGC → Pulsar-Native → Plugin_Blueprints → ui_core)
