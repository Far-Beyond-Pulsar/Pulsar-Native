# Blueprint Node Registry System

## Overview

The blueprint node registration system now uses **compile-time macro-based registration** instead of runtime source parsing. This makes it:

- ✅ **Modular**: Works with `pub mod` + `pub use` pattern
- ✅ **Fast**: No runtime parsing, registry built at compile time
- ✅ **Type-safe**: All metadata is statically verified
- ✅ **Automatic**: No manual registration needed

## How It Works

### 1. Macro Auto-Registration

When you write a blueprint node:

```rust
#[blueprint(type: NodeTypes::pure, category: "Math", color: "#4A90E2")]
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}
```

The `#[blueprint]` macro automatically:
1. Keeps your function unchanged
2. Generates a static `NodeMetadata` const
3. Registers it in the global `BLUEPRINT_REGISTRY` using `linkme`

### 2. Distributed Collection

The `linkme` crate collects all nodes at link time:

```rust
#[distributed_slice]
pub static BLUEPRINT_REGISTRY: [NodeMetadata] = [..];
```

This works across:
- Multiple modules
- Multiple files
- The entire crate

### 3. Engine Access

The engine simply imports the pre-built registry:

```rust
use pulsar_std::get_all_nodes;

let nodes = get_all_nodes(); // All nodes, from everywhere
```

## Module Organization

The system works seamlessly with module organization:

```rust
// lib.rs
pub mod math;
pub use math::*;

pub mod logic;
pub use logic::*;
```

Each module's nodes are automatically collected, regardless of how they're imported or re-exported.

## Registry API

```rust
// Get all nodes
let all_nodes = pulsar_std::get_all_nodes();

// Get nodes by category
let math_nodes = pulsar_std::get_nodes_by_category("Math");

// Get specific node
let add_node = pulsar_std::get_node_by_name("add");

// Get all categories
let categories = pulsar_std::get_all_categories();
```

## Node Metadata Structure

```rust
pub struct NodeMetadata {
    pub name: &'static str,
    pub node_type: NodeType,
    pub params: &'static [NodeParameter],
    pub return_type: Option<&'static str>,
    pub exec_inputs: &'static [&'static str],
    pub exec_outputs: &'static [&'static str],
    pub function_source: &'static str,
    pub documentation: &'static [&'static str],
    pub category: &'static str,
    pub color: Option<&'static str>,
}
```

All data is `&'static` - embedded in the binary at compile time.

## Migration from Old System

### Old System (Deprecated)
- ❌ Parsed `pulsar_std/src/lib.rs` at compile time using `include_str!`
- ❌ Used `syn` to find `#[blueprint]` functions
- ❌ Didn't work well with module organization
- ❌ Required all nodes in one file

### New System
- ✅ Macro generates registry entries
- ✅ Works with any module structure
- ✅ Automatic collection with `linkme`
- ✅ Zero-overhead at runtime

## Dependencies

- **linkme** (0.3): Distributed slice collection for the registry
- **pulsar_macros**: Provides the `#[blueprint]` procedural macro

## Adding New Nodes

Just use the `#[blueprint]` attribute - registration is automatic:

```rust
// anywhere in pulsar_std or its modules
#[bp_doc("# My Node")]
#[bp_doc("Does something cool")]
#[blueprint(type: NodeTypes::pure, category: "Custom")]
pub fn my_node(x: i64) -> i64 {
    x * 2
}
```

No additional steps needed - it's automatically in the registry!
