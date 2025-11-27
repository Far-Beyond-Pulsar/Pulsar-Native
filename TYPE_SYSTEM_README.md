# Pulsar Type System Editors

A visual type definition system for Pulsar projects, allowing you to create and manage Rust types (structs, enums, traits, and type aliases) through GPUI-based editors.

## Overview

The Type System consists of four integrated editors that work within the Pulsar Engine:

- **📦 Struct Editor** - Define data structures with fields
- **🎯 Enum Editor** - Create enumerations with variants
- **🎭 Trait Editor** - Define traits with method signatures
- **🔗 Alias Editor** - Build complex type aliases using an AST-based system

## Architecture

### Components

1. **ui_types_common** - Shared type definitions, validation, and code generation
2. **ui_struct_editor** - Struct editor UI crate
3. **ui_enum_editor** - Enum editor UI crate
4. **ui_trait_editor** - Trait editor UI crate
5. **ui_alias_editor** - Type alias editor UI crate

### Project Structure

When you open a Pulsar project, type definitions are stored in:

```
YourProject/
├── Pulsar.toml
├── type-index/
│   └── index.json          # Central type index
└── types/
    ├── structs/
    │   └── inventory_item/
    │       ├── struct.json  # Type definition (JSON)
    │       └── mod.rs       # Generated Rust code
    ├── enums/
    │   └── damage_type/
    │       ├── enum.json
    │       └── mod.rs
    ├── traits/
    │   └── drawable/
    │       ├── trait.json
    │       └── mod.rs
    └── aliases/
        └── boxed_item/
            ├── alias.json
            └── mod.rs
```

## Type Reference System

### Allowed Type References

In struct fields, enum payloads, and trait method parameters, you can use:

- **Primitives**: `i32`, `u64`, `String`, `bool`, `f64`, etc.
- **Path types**: `std::time::Duration`, `std::path::PathBuf`, etc.
- **Alias references**: References to type aliases you've created

### Restriction: No Direct Constructors

You **cannot** use complex wrapper types like `Box<T>`, `Arc<T>`, `Result<T, E>`, `Vec<T>` directly in fields.

Instead, create a **type alias** first:

1. Open the Alias Editor
2. Create an alias like `boxed_item` with AST: `Box<Item>`
3. Then use `boxed_item` in your struct fields

This ensures proper dependency tracking and prevents accidental complexity.

## Type Index (index.json)

The central `type-index/index.json` file tracks all types:

```json
{
  "schemaVersion": 1,
  "generatedAt": "2025-11-22T12:00:00Z",
  "types": {
    "aliases": {
      "boxed_item": {
        "id": "alias:boxed_item",
        "name": "boxed_item",
        "displayName": "BoxedItem",
        "jsonFile": "types/aliases/boxed_item/alias.json",
        "rsFile": "types/aliases/boxed_item/mod.rs",
        "lastCompiled": "2025-11-21T15:20:00Z",
        "version": 1
      }
    },
    "structs": { /* ... */ },
    "enums": { /* ... */ },
    "traits": { /* ... */ }
  }
}
```

## Workflow Example

### Creating a Struct

1. **Open the File Drawer** in the Blueprint Editor
2. **Navigate to Types mount point** (📐 Types)
3. **Create new struct file**: Right-click → New → Struct
4. **Edit in Struct Editor**:
   - Set name: `inventory_item`
   - Set display name: `InventoryItem`
   - Add fields:
     - `id: u64`
     - `name: String`
     - `metadata: item_metadata` (reference to alias)
5. **Save** - This will:
   - Write `types/structs/inventory_item/struct.json`
   - Generate `types/structs/inventory_item/mod.rs`
   - Update `type-index/index.json`

Generated Rust code:

```rust
// AUTO-GENERATED. DO NOT EDIT MANUALLY.
#![allow(dead_code)]

pub struct InventoryItem {
    pub id: u64,
    pub name: String,
    pub metadata: ItemMetadata,
}
```

### Creating a Type Alias

1. Open Alias Editor
2. Build type using AST constructor graph:
   - Constructor: `Box`
     - Param: Constructor: `Arc`
       - Param: Constructor: `Result`
         - Param 1: AliasRef: `metadata`
         - Param 2: AliasRef: `metadata_error`
3. Save as `boxed_metadata`

Generated code:

```rust
// AUTO-GENERATED. DO NOT EDIT MANUALLY.
#![allow(dead_code)]

pub type BoxedMetadata = Box<Arc<Result<Metadata, MetadataError>>>;
```

## Validation

The system validates:

- ✓ Name format: `^[A-Za-z_][A-Za-z0-9_]*$`
- ✓ No name collisions across type kinds
- ✓ Type references exist (aliases, primitives, paths)
- ✓ Generic constructor arity (e.g., `Result` needs 2 params)
- ✓ No circular dependencies in aliases
- ✓ Proper visibility modifiers

## Code Generation

On save, each editor:

1. **Validates** the type definition
2. **Generates** Rust code with proper formatting
3. **Writes** `.json` and `.rs` files
4. **Updates** the type index with version++

The generated Rust files are auto-formatted and include:
- Doc comments from descriptions
- Proper visibility modifiers
- Full type resolution

## Integration with Engine

Type editors integrate seamlessly:

- **File Drawer**: Browse types under "Types" mount point
- **Tab System**: Editors open in main tab bar
- **Save/Load**: JSON persistence with Rust code generation
- **Type Picker**: Select from available types when building

## File Extensions

- `.struct.json` - Opens Struct Editor
- `.enum.json` - Opens Enum Editor
- `.trait.json` - Opens Trait Editor
- `.alias.json` - Opens Alias Editor

## Future Enhancements

- [ ] Visual AST editor for aliases (node graph)
- [ ] Type browser/palette
- [ ] Import existing Rust types
- [ ] Cross-project type libraries
- [ ] CI integration for type-checking
- [ ] Struct/enum derive macro configuration
- [ ] Trait implementation templates

## Technical Details

### JSON Schema

All type assets follow a versioned schema:

```json
{
  "schemaVersion": 1,
  "typeKind": "struct|enum|trait|alias",
  "name": "snake_case_name",
  "displayName": "PascalCaseName",
  "description": "Optional description",
  // ... type-specific fields
}
```

### AST Node Format (Aliases)

```json
{
  "nodeKind": "Constructor",
  "name": "Box",
  "params": [
    { "nodeKind": "Primitive", "name": "u64" }
  ],
  "lifetimes": [],
  "const_generics": []
}
```

### Versioning

Each save increments the version number in the index:

- Version tracking prevents concurrent edit conflicts
- `lastCompiled` timestamp shows when Rust code was generated
- Index can be used for cache invalidation

## Usage with Existing Code

To use generated types in your Rust code:

```rust
// In your project's main.rs or lib.rs
pub mod types;

use types::structs::inventory_item::InventoryItem;
use types::aliases::boxed_metadata::BoxedMetadata;

fn process_item(item: InventoryItem) {
    println!("Processing: {}", item.name);
}
```

## Troubleshooting

**Types not appearing in picker:**
- Ensure type is saved and index.json updated
- Check type-index/index.json exists
- Verify project root is correctly set

**Generated Rust doesn't compile:**
- Check all type references exist
- Verify no circular dependencies
- Ensure proper use of lifetimes in aliases

**Save fails:**
- Validate type name follows naming rules
- Check for name collisions
- Ensure no missing alias references

## Contributing

The type system is modular and extensible:

- Add new type kinds by creating new editor crates
- Extend validation in `ui_types_common/src/validation.rs`
- Customize code generation in `ui_types_common/src/codegen.rs`

---

**Built with GPUI** for the Pulsar Engine ecosystem.
