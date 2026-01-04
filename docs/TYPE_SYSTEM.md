# Type System Reference

Pulsar Engine includes a custom type system for game data. This document explains how types work, how to define them, and how they're used throughout the engine.

## Overview

The type system provides:

- Strongly typed game data
- Runtime type introspection
- Editor validation and autocomplet
- Cross-file type references
- Standard library of common types

Types are defined in Rust and stored in a central Type Database. The editor uses this database to validate data files and provide IntelliSense-like features.

## Type Categories

### Structs

Data structures with named fields.

**Definition:**
```rust
pub struct Player {
    pub name: String,
    pub health: f32,
    pub position: Vector3,
}
```

**Usage in Data:**
```json
{
    "type": "Player",
    "data": {
        "name": "Alice",
        "health": 100.0,
        "position": [0.0, 0.0, 0.0]
    }
}
```

### Enums

Tagged unions with variants.

**Definition:**
```rust
pub enum GameState {
    Menu,
    Playing { level: u32 },
    Paused,
    GameOver { score: u32 },
}
```

**Usage in Data:**
```json
{
    "type": "GameState",
    "variant": "Playing",
    "data": {
        "level": 5
    }
}
```

### Traits

Interfaces defining behavior contracts.

**Definition:**
```rust
pub trait Damageable {
    fn take_damage(&mut self, amount: f32);
    fn is_alive(&self) -> bool;
}
```

Traits are used for compile-time polymorphism and editor type checking.

### Type Aliases

Named shortcuts for complex types.

**Definition:**
```rust
pub type EntityId = u64;
pub type Transform = (Vector3, Quaternion, Vector3);
```

Aliases improve readability without creating new types.

## Built-in Types

Pulsar's standard library (`pulsar_std`) provides common types:

### Primitives
- `bool` - Boolean
- `i8, i16, i32, i64, i128` - Signed integers
- `u8, u16, u32, u64, u128` - Unsigned integers
- `f32, f64` - Floating point
- `char` - Unicode character
- `String` - UTF-8 string

### Collections
- `Vec<T>` - Dynamic array
- `HashMap<K, V>` - Hash map
- `HashSet<T>` - Hash set
- `Option<T>` - Optional value
- `Result<T, E>` - Success or error

### Math Types
- `Vector2` - 2D vector
- `Vector3` - 3D vector
- `Vector4` - 4D vector
- `Quaternion` - Rotation
- `Matrix4` - 4x4 matrix
- `Color` - RGBA color

### Engine Types
- `EntityId` - Entity identifier
- `AssetHandle<T>` - Asset reference
- `Transform` - Position, rotation, scale
- `Time` - Time and duration

## Type Database

The Type Database stores all project types and provides query APIs.

### Querying Types

```rust
use type_db::TypeDatabase;

// Get type by name
let player_type = type_db.get_type("Player")?;

// Get all types
let all_types = type_db.all();

// Get types by kind
let structs = type_db.get_by_kind(TypeKind::Struct);

// Search types
let results = type_db.search("Player");
```

### Type Information

```rust
pub struct TypeInfo {
    pub name: String,
    pub kind: TypeKind,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
    pub visibility: Visibility,
}

pub enum TypeKind {
    Struct,
    Enum,
    Trait,
    Alias,
}

pub enum Visibility {
    Public,
    Private,
    Crate,
}
```

### Registering Types

Types auto-register when parsed from Rust files:

```rust
// src/types/player.rs
pub struct Player {  // Automatically registered
    // ...
}
```

Manual registration:

```rust
type_db.register_with_path(
    "CustomType".into(),
    PathBuf::from("src/custom.rs"),
    TypeKind::Struct,
    Visibility::Public,
    vec![],  // Dependencies
    None,    // Generic constraints
)?;
```

## Type Resolution

The type system resolves references between types:

```rust
pub struct Inventory {
    pub items: Vec<Item>,  // References Item type
}

pub struct Item {
    pub name: String,
}
```

Resolution process:
1. Parse type definitions
2. Build dependency graph
3. Check for cycles
4. Validate references exist

## Using Types in Data Files

### JSON Data Files

```json
{
    "$type": "Player",
    "name": "Alice",
    "health": 100.0,
    "inventory": {
        "$type": "Inventory",
        "items": [
            {
                "$type": "Item",
                "name": "Sword"
            }
        ]
    }
}
```

The `$type` field specifies which type to use. The editor validates fields against the type definition.

### TOML Configuration

```toml
[player]
type = "Player"
name = "Bob"
health = 75.0

[[player.inventory.items]]
type = "Item"
name = "Shield"
```

TOML provides a more readable format for structured data.

## Editor Integration

### Type Validation

The editor validates data files against types:

```rust
// Check if value matches type
fn validate(value: &JsonValue, type_info: &TypeInfo) -> Result<(), ValidationError> {
    match type_info.kind {
        TypeKind::Struct => validate_struct(value, type_info),
        TypeKind::Enum => validate_enum(value, type_info),
        // ...
    }
}
```

Validation errors appear in the Problems panel.

### Autocomplete

Type information powers IntelliSense:

```json
{
    "$type": "Player",
    "na|"  // Autocomplete suggests: name, health, position
}
```

The editor queries the Type Database for available fields.

### Type Inspection

The Type Debugger shows:
- All project types
- Type details (fields, methods)
- Type relationships
- Where types are used

Access via: Problems panel → Type Debugger tab

## Type System Implementation

### Parsing

Rust types are parsed from source using `syn`:

```rust
use syn::{ItemStruct, ItemEnum, ItemTrait};

fn parse_struct(item: &ItemStruct) -> TypeInfo {
    TypeInfo {
        name: item.ident.to_string(),
        kind: TypeKind::Struct,
        // ... extract fields
    }
}
```

### Storage

Types stored in SQLite database:

```sql
CREATE TABLE types (
    name TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    path TEXT NOT NULL,
    visibility TEXT,
    data BLOB  -- Serialized type details
);
```

### Caching

Hot types are cached in memory:

```rust
pub struct TypeCache {
    types: HashMap<String, Arc<TypeInfo>>,
    // ...
}
```

## Advanced Features

### Generic Types

```rust
pub struct Container<T> {
    pub value: T,
}

// Usage
let int_container: Container<i32>;
let player_container: Container<Player>;
```

Generics work like Rust generics with monomorphization.

### Constraints

```rust
pub struct Wrapper<T>
where
    T: Damageable + Clone
{
    pub inner: T,
}
```

Constraints enforce trait bounds.

### Derive Macros

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
}
```

Common traits can be auto-derived.

## Performance Considerations

### Type Database Queries

- Hot types: <1ms lookup
- Cold types: ~10ms from database
- Full scan: ~100ms for 1000 types

### Memory Usage

- Per type: ~1KB
- 1000 types: ~1MB
- Cache size: Configurable

### Optimization Tips

1. **Batch queries**: Use `get_many()` instead of multiple `get()`
2. **Cache results**: Store `Arc<TypeInfo>` to avoid re-querying
3. **Index by path**: Fast lookup for file-based queries

## Common Patterns

### Component Pattern

```rust
pub struct Transform {
    pub position: Vector3,
    pub rotation: Quaternion,
    pub scale: Vector3,
}

pub struct Renderable {
    pub mesh: AssetHandle<Mesh>,
    pub material: AssetHandle<Material>,
}
```

ECS components are types like any other.

### Builder Pattern

```rust
pub struct PlayerBuilder {
    name: String,
    health: f32,
}

impl PlayerBuilder {
    pub fn new() -> Self { /* ... */ }
    pub fn with_name(mut self, name: String) -> Self { /* ... */ }
    pub fn build(self) -> Player { /* ... */ }
}
```

Builders create complex types step-by-step.

### Newtype Pattern

```rust
pub struct PlayerId(u64);
pub struct ItemId(u64);
```

Newtypes prevent mixing up similar types.

## Error Handling

### Type Errors

```rust
pub enum TypeError {
    NotFound { name: String },
    InvalidKind { expected: TypeKind, found: TypeKind },
    CircularDependency { chain: Vec<String> },
    ValidationError { field: String, message: String },
}
```

All type errors implement `std::error::Error`.

### Error Recovery

The editor attempts to recover from type errors:

- Missing fields: Use default values
- Unknown types: Show placeholder
- Invalid data: Highlight issue

## Best Practices

### Naming

- **PascalCase** for types: `PlayerController`
- **snake_case** for fields: `player_name`
- **SCREAMING_SNAKE_CASE** for constants: `MAX_HEALTH`

### Organization

```
src/
├── types/
│   ├── mod.rs
│   ├── player.rs
│   ├── enemy.rs
│   └── items.rs
├── components/
└── systems/
```

Group related types in modules.

### Documentation

```rust
/// Represents a player character.
///
/// Players have health, inventory, and can move around the world.
pub struct Player {
    /// The player's display name
    pub name: String,
    
    /// Current health points (0-100)
    pub health: f32,
}
```

Document public types and fields.

### Versioning

```rust
#[derive(Serialize, Deserialize)]
pub struct SaveData {
    pub version: u32,  // Track data version
    pub player: Player,
}
```

Include version fields for save data.

## Integration with Other Systems

### Asset System

```rust
pub struct Texture {
    id: AssetHandle<TextureData>,
}
```

Types can reference assets via handles.

### ECS System

```rust
// Component is a type
struct Position(Vector3);

// System queries types
fn movement_system(query: Query<&mut Position>) {
    // ...
}
```

### Serialization

```rust
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct Config {
    // Automatically serializes/deserializes
}
```

## Debugging Types

### Type Debugger UI

View all types in the editor:
1. Open Type Debugger (bottom panel)
2. Browse type list
3. Click type for details
4. See where type is used

### Console Commands

```rust
// Print type info
:type Player

// List all types
:types

// Search types
:type-search "Player"
```

### Logging

```rust
use tracing::debug;

debug!("Registered type: {:?}", type_info);
```

## Limitations

Current limitations:

- No runtime type creation
- No type modification at runtime
- Limited generic type support
- No procedural macros in data

Future improvements planned.

## Migration Guide

### From 0.1.x to 0.2.x

Breaking changes:

1. Type paths now required
2. Visibility now tracked
3. New validation rules

Update code:

```rust
// Old
type_db.register("MyType", TypeKind::Struct)?;

// New
type_db.register_with_path(
    "MyType".into(),
    path,
    TypeKind::Struct,
    Visibility::Public,
    vec![],
    None,
)?;
```

## Related Documentation

- [Architecture Overview](ARCHITECTURE.md)
- [Plugin Development](PLUGIN_DEVELOPMENT.md)
- [Data File Formats](DATA_FORMATS.md)

## Troubleshooting

### Type Not Found

- Check type is defined in project
- Verify file is in `src/` directory
- Ensure type is `pub` if used externally

### Validation Errors

- Check data matches type definition
- Verify required fields present
- Ensure types referenced exist

### Performance Issues

- Check type cache size
- Profile database queries
- Consider indexing strategy

---

**See Also:**
- Type Database implementation: `/crates/type_db`
- Standard library: `/crates/pulsar_std`
- Example types: `/examples/type-system-demo`
