# Pulsar Type System - Block-Based Alias Editor

## Summary

The Pulsar Type System now features a **Scratch-like visual block editor** for creating complex type aliases, with type constructors dynamically loaded from a compile-time registry using the `#[blueprint_type]` macro.

## Key Changes

### 1. Block-Based Visual Editor

Instead of manually editing JSON or typing angle brackets, users build types visually:

```
Visual:                          Code:
┌──────────────────┐            Arc<RwLock<Entity>>
│ Arc<             │
│  ┌────────────┐  │
│  │ RwLock<    │  │
│  │  ┌──────┐  │  │
│  │  │Entity│  │  │
│  │  └──────┘  │  │
│  │ >          │  │
│  └────────────┘  │
│ >                │
└──────────────────┘
```

### 2. Macro-Based Type Registration

Type constructors are registered using `#[blueprint_type]` just like blueprint nodes:

```rust
// In pulsar_std/src/type_constructors.rs

#[blueprint_type(
    params: 1,
    category: "Smart Pointers",
    description: "Thread-safe reference counting",
    unwrapped_name: "Arc"
)]
pub type PArc<T> = Arc<T>;
```

**Key Design:** Types are **actual usable type aliases** (PArc, PBox, PVec), not dummy structs!

### 3. Dynamic Constructor Palette

The palette loads from `TYPE_CONSTRUCTOR_REGISTRY` at runtime:

```rust
use pulsar_std::get_all_type_constructors;

let constructors = get_all_type_constructors();
// Dynamically populate UI categories
```

No hardcoding! Add a new type constructor anywhere and it automatically appears in the editor.

## Architecture

### Macro System (`pulsar_macros`)

```rust
#[proc_macro_attribute]
pub fn blueprint_type(args: TokenStream, input: TokenStream) -> TokenStream {
    // Parses: pub type PArc<T> = Arc<T>
    // Extracts: unwrapped_name = "Arc", params = 1, category, etc.
    // Generates: Static registry entry via linkme
}
```

### Type Registry (`pulsar_std/registry.rs`)

```rust
#[distributed_slice]
pub static TYPE_CONSTRUCTOR_REGISTRY: [TypeConstructorMetadata] = [..];

pub struct TypeConstructorMetadata {
    pub name: &'static str,           // "Arc"
    pub params_count: usize,          // 1
    pub category: &'static str,       // "Smart Pointers"
    pub description: &'static str,    // "Thread-safe reference counting"
    pub example: &'static str,        // "Arc<T>"
}
```

### Visual Components (`ui_alias_editor`)

**TypeBlock** (`type_block.rs`)
- Leaf blocks: Primitives (blue), Paths (green), Aliases (purple)
- Container blocks: Constructors (orange), Tuples (yellow)
- Bi-directional conversion: `TypeBlock ↔ TypeAstNode`

**ConstructorPalette** (`constructor_palette.rs`)
- Loads constructors from `TYPE_CONSTRUCTOR_REGISTRY`
- Groups by category with icons
- Clickable blocks to add to canvas

**AliasEditor** (`editor.rs`)
- Three-panel layout: Palette | Canvas | Preview
- Quick action buttons for common types
- Live Rust code preview
- Saves to JSON + generates .rs file

## Workflow

### Adding a New Type Constructor

1. **Define in pulsar_std:**
```rust
#[blueprint_type(params: 2, category: "Collections", description: "Linked list", unwrapped_name: "LinkedList")]
pub type PLinkedList<T> = LinkedList<T>;
```

2. **Compile** - Type is automatically added to registry

3. **Open alias editor** - New "LinkedList" block appears in Collections category!

### Creating a Type Alias Visually

1. Open alias editor (click .alias.json in file drawer)
2. Click constructor from palette (or quick button)
3. Fill slots with nested types
4. Watch live preview update
5. Save → Generates Rust code + updates index

### Example: Building `Arc<Mutex<Vec<String>>>`

**Steps:**
1. Click "+ Arc" → Creates Arc block with 1 slot
2. In Arc's slot, click "Mutex" from palette
3. In Mutex's slot, click "Vec" from palette
4. In Vec's slot, select "String" primitive
5. Save!

**Visual Result:**
```
┌────────────────────┐
│ Arc<               │
│  ┌──────────────┐  │
│  │ Mutex<       │  │
│  │  ┌────────┐  │  │
│  │  │ Vec<   │  │  │
│  │  │  ┌───┐ │  │  │
│  │  │  │Str│ │  │  │
│  │  │  └───┘ │  │  │
│  │  │ >      │  │  │
│  │  └────────┘  │  │
│  │ >            │  │
│  └──────────────┘  │
│ >                  │
└────────────────────┘
```

**Generated Code:**
```rust
// AUTO-GENERATED
pub type MyAlias = Arc<Mutex<Vec<String>>>;
```

## Benefits

✅ **No hardcoded types** - Uses macro registry like blueprint nodes
✅ **Visual construction** - Scratch-like block interface
✅ **Extensible** - Add new constructors anywhere in codebase
✅ **Type-safe** - Validates parameter counts and references
✅ **Usable types** - PArc, PBox, etc. are real type aliases you can import
✅ **Auto-discovery** - New types appear in editor automatically

## File Structure

```
crates/
  pulsar_macros/src/lib.rs        # #[blueprint_type] macro
  pulsar_std/
    src/registry.rs               # TYPE_CONSTRUCTOR_REGISTRY
    src/type_constructors.rs      # PArc, PBox, PVec, etc.

ui-crates/
  ui_alias_editor/
    src/
      type_block.rs               # Visual block component
      constructor_palette.rs      # Dynamic palette from registry
      editor.rs                   # Main editor UI

  ui_types_common/
    src/
      types.rs                    # TypeAstNode, TypeRef
      codegen.rs                  # AST → Rust code
      validation.rs               # Type validation
```

## Comparison to Blueprint Nodes

| Blueprint Nodes | Type Constructors |
|----------------|-------------------|
| `#[blueprint]` | `#[blueprint_type]` |
| `BLUEPRINT_REGISTRY` | `TYPE_CONSTRUCTOR_REGISTRY` |
| `NodeMetadata` | `TypeConstructorMetadata` |
| Function definitions | Type aliases |
| Node graph editor | Block-based editor |
| `get_all_nodes()` | `get_all_type_constructors()` |

**Same pattern, different domain!**

## Future Enhancements

- [ ] Drag-and-drop from palette to canvas
- [ ] Visual slot highlighting on hover
- [ ] Block templates (common patterns like `Arc<RwLock<T>>`)
- [ ] Search/filter in palette
- [ ] Lifetime annotation UI
- [ ] Generic constraints UI (where T: Clone)
- [ ] Import user types from project code

## Usage in Projects

Users can import and use the P-prefixed types:

```rust
use pulsar_std::{PArc, PMutex, PVec};

type SharedData = PArc<PMutex<PVec<String>>>;

fn process(data: SharedData) {
    // Works exactly like Arc<Mutex<Vec<String>>>
}
```

Or define custom aliases in the visual editor that generate similar code.

---

**No more angle bracket hell!** Build complex Rust types with visual blocks.
