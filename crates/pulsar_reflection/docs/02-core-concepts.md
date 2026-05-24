# Core Concepts - Understanding the Reflection System

This guide explains the fundamental concepts and architecture of the Pulsar reflection system. By understanding how the pieces fit together, you'll be able to use reflection effectively and debug issues when they arise.

## The Big Picture

The reflection system bridges two worlds:

1. **Compile Time**: When Rust compiles your code, it knows everything about your types—their names, sizes, fields, memory layout, and relationships.

2. **Runtime**: After compilation, this information is normally lost. Your program executes but has no way to introspect types dynamically.

The reflection system captures compile-time information and makes it available at runtime through static data structures that live for the entire program duration.

## Core Data Structures

### RuntimeTypeInfo - The Type Descriptor

This is the central structure that describes a type:

```rust
pub struct RuntimeTypeInfo {
    pub type_id: TypeId,           // Unique identifier from std::any
    pub type_name: &'static str,   // Human-readable name
    pub size: usize,               // Size in bytes
    pub align: usize,              // Alignment requirement
    pub structure: TypeStructure,  // Structural information
}
```

Every field is `'static` because it's computed at compile time and stored in the binary's data section. No runtime allocation is needed.

#### TypeId - The Unique Identifier

The `TypeId` comes from Rust's standard library (`std::any::TypeId`). It's a unique, opaque identifier for each type:

```rust
let f32_id = TypeId::of::<f32>();
let i32_id = TypeId::of::<i32>();

assert_ne!(f32_id, i32_id);  // Different types have different IDs
```

Two types have the same `TypeId` if and only if they're the same type. This is how the system performs safe type checking at runtime:

```rust
fn can_downcast(value: &dyn Any, type_info: &RuntimeTypeInfo) -> bool {
    value.type_id() == type_info.type_id
}
```

The comparison is just a pointer-sized integer comparison—extremely fast.

#### TypeStructure - Describing the Shape

The `TypeStructure` enum describes what kind of type this is and its internal organization:

```rust
pub enum TypeStructure {
    Primitive,
    String,
    Wrapper {
        wrapper_kind: WrapperType,
        inner: &'static RuntimeTypeInfo,
    },
    Struct {
        fields: &'static [FieldInfo],
    },
    Enum {
        variants: &'static [&'static str],
    },
}
```

**Primitive**: Simple scalar types like `f32`, `i32`, `bool`. No internal structure to describe.

**String**: The `String` type gets special treatment because it's so common.

**Wrapper**: Container types like `Vec<T>`, `Option<T>`, `Arc<T>`. The `wrapper_kind` identifies the container, and `inner` points to the element type's descriptor. This allows recursive type descriptions.

**Struct**: Contains an array of `FieldInfo` describing each field:

```rust
pub struct FieldInfo {
    pub name: &'static str,
    pub type_info: &'static RuntimeTypeInfo,
    pub offset: usize,
}
```

The `offset` tells you where the field is located in memory relative to the start of the struct. This is computed using `std::mem::offset_of!` at compile time.

**Enum**: Contains an array of variant names. Currently, we only track simple enums (no associated data).

### RuntimeTypeRegistry - The Global Catalog

The registry is a global singleton that stores all registered types:

```rust
pub struct RuntimeTypeRegistry {
    types: HashMap<TypeId, &'static RuntimeTypeInfo>,
    by_name: HashMap<&'static str, &'static RuntimeTypeInfo>,
}

pub static RUNTIME_TYPE_REGISTRY: LazyLock<RuntimeTypeRegistry>;
```

It provides two indices for fast lookup:

1. **By TypeId**: O(1) lookup when you know the type statically
2. **By name**: O(1) lookup when you have a string (from JSON, user input, etc.)

The registry is populated at link time through the `inventory` crate, which collects registrations from across your entire program.

## How Types Get Registered

When you write `#[derive(Reflectable)]`, the proc macro generates three things:

### 1. The Static Descriptor

```rust
static PLAYER_POSITION_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<PlayerPosition>(),
    type_name: "PlayerPosition",
    size: std::mem::size_of::<PlayerPosition>(),
    align: std::mem::align_of::<PlayerPosition>(),
    structure: TypeStructure::Struct {
        fields: &[
            FieldInfo {
                name: "x",
                type_info: /* recursive lookup for f32 */,
                offset: 0,
            },
            // ... more fields
        ],
    },
};
```

This is computed entirely at compile time using `const fn` evaluation. No runtime cost.

### 2. The Registration

```rust
inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &PLAYER_POSITION_TYPE_INFO,
    }
}
```

The `inventory` crate collects these submissions at link time. When your program starts, they're already available—no init function needed.

### 3. The Trait Implementation

```rust
impl Reflectable for PlayerPosition {
    fn type_info() -> &'static RuntimeTypeInfo {
        &PLAYER_POSITION_TYPE_INFO
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        // Generated serialization code
    }

    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self> {
        // Generated deserialization code
    }

    fn clone_any(&self) -> Box<dyn Any> {
        Box::new(self.clone())
    }
}
```

This implementation allows generic code to work with your type through the `Reflectable` trait bound.

## The Reflectable Trait

The trait has four key methods:

```rust
pub trait Reflectable: Any + Send + Sync {
    fn type_info() -> &'static RuntimeTypeInfo;
    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()>;
    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self>;
    fn clone_any(&self) -> Box<dyn Any>;
}
```

### type_info() - Access Metadata

This is a static method (no `self`) that returns the type's descriptor. You can call it without an instance:

```rust
let info = PlayerPosition::type_info();
println!("Size: {}", info.size);
```

### serialize() - Convert to External Format

Uses the visitor pattern through `TypeSerializer` to convert the value to an external representation:

```rust
let position = PlayerPosition { x: 1.0, y: 2.0, z: 3.0 };
let mut serializer = JsonSerializer::new();
position.serialize(&mut serializer)?;
let json = serializer.into_json();
```

The serializer visits each field and converts it to JSON (or whatever format it implements).

### deserialize() - Reconstruct from External Format

The inverse operation, using `TypeDeserializer`:

```rust
let json = serde_json::json!({
    "x": 1.0,
    "y": 2.0,
    "z": 3.0
});
let mut deserializer = JsonDeserializer::new(json);
let position = PlayerPosition::deserialize(&mut deserializer)?;
```

### clone_any() - Type-Erased Cloning

Returns a `Box<dyn Any>` containing a clone of the value. This enables generic cloning in reflection-based code:

```rust
fn clone_reflectable<T: Reflectable>(value: &T) -> Box<dyn Any> {
    value.clone_any()
}
```

## Type Erasure with Any

The `std::any::Any` trait is crucial for runtime typing:

```rust
pub trait Any {
    fn type_id(&self) -> TypeId;
}
```

When you have `Box<dyn Any>`, you can safely downcast to concrete types:

```rust
let boxed: Box<dyn Any> = Box::new(42f32);

// Safe downcasting
if let Some(value) = boxed.downcast_ref::<f32>() {
    println!("It's an f32: {}", value);
} else {
    println!("Not an f32");
}
```

The downcast compares `TypeId`s internally. If they don't match, you get `None`—never undefined behavior.

### Why Box<dyn Any> Instead of Generic T?

Property systems need to store values of different types in a single collection:

```rust
// This doesn't work - can't mix types
Vec<T>  // T must be one type

// This works - type-erased storage
Vec<Box<dyn Any>>  // Can store any type
```

The trade-off is that you must downcast to get the concrete value back:

```rust
let values: Vec<Box<dyn Any>> = vec![
    Box::new(42f32),
    Box::new("hello".to_string()),
    Box::new(true),
];

// Extract the f32
if let Some(num) = values[0].downcast_ref::<f32>() {
    println!("Number: {}", num);
}
```

## Memory Layout and Safety

### Field Offsets

When the macro generates `FieldInfo`, it calculates the byte offset of each field from the struct's start:

```rust
struct Example {
    a: f32,   // offset 0, size 4
    b: i32,   // offset 4, size 4
    c: bool,  // offset 8, size 1
}
```

This is done using `std::mem::offset_of!()`, which is safe because it uses compiler intrinsics. The offset is computed at compile time and stored as a constant.

**Critical**: The offset is for **debug purposes only**. Never use it for raw pointer arithmetic. Use the property getters/setters instead, which handle everything safely.

### Alignment and Padding

Rust automatically adds padding to maintain alignment requirements:

```rust
struct Padded {
    a: u8,    // 1 byte
    // 3 bytes padding here
    b: u32,   // 4 bytes, must be aligned to 4
}
```

The reflection system captures the total size including padding:

```rust
let info = Padded::type_info();
assert_eq!(info.size, 8);  // 1 + 3 padding + 4
assert_eq!(info.align, 4);  // Alignment of largest field
```

The `TypeStructure::Struct` contains field offsets that account for padding.

## Serialization Architecture

The serialization system uses traits for extensibility:

### TypeSerializer - Writing Values

```rust
pub trait TypeSerializer {
    fn serialize_f32(&mut self, value: f32);
    fn serialize_i32(&mut self, value: i32);
    fn serialize_string(&mut self, value: &str);
    fn serialize_struct(&mut self, fields: &[(&str, &dyn Any)]);
    // ... more methods
}
```

Implementations can target different formats:

- `JsonSerializer` - Converts to `serde_json::Value`
- `BinarySerializer` - Hypothetical binary format
- `NetworkSerializer` - Network protocol encoding

### TypeDeserializer - Reading Values

```rust
pub trait TypeDeserializer {
    fn deserialize_f32(&mut self) -> ReflectResult<f32>;
    fn deserialize_i32(&mut self) -> ReflectResult<i32>;
    fn deserialize_string(&mut self) -> ReflectResult<String>;
    fn deserialize_struct(&mut self, fields: &[FieldInfo]) -> ReflectResult<HashMap<&'static str, Box<dyn Any>>>;
    // ... more methods
}
```

The deserializer uses the `RuntimeTypeInfo` to validate structure and create instances.

### The Pattern

```rust
// Serialization: Value → Serializer → External Format
let mut serializer = JsonSerializer::new();
value.serialize(&mut serializer)?;
let json = serializer.into_json();

// Deserialization: External Format → Deserializer → Value
let mut deserializer = JsonDeserializer::new(json);
let value = Type::deserialize(&mut deserializer)?;
```

This visitor pattern separates the "what" (your type structure) from the "how" (the format).

## PropertyMetadata - Engine Integration

The `EngineClass` system builds on reflection to provide editor integration:

```rust
pub struct PropertyMetadata {
    pub name: &'static str,
    pub display_name: String,
    pub category: Option<&'static str>,
    pub type_info: &'static RuntimeTypeInfo,
    pub getter: Box<dyn Fn(&dyn EngineClass) -> Box<dyn Any> + Send + Sync>,
    pub setter: Box<dyn Fn(&mut dyn EngineClass, Box<dyn Any>) + Send + Sync>,
}
```

The `getter` and `setter` are type-erased closures that know how to access fields on specific component types. The `EngineClass` derive macro generates these automatically:

```rust
#[derive(EngineClass, Reflectable)]
pub struct HealthComponent {
    #[property]
    pub current_health: f32,
}

// Macro generates:
impl EngineClass for HealthComponent {
    fn get_properties(&self) -> Vec<PropertyMetadata> {
        vec![
            PropertyMetadata {
                name: "current_health",
                display_name: "Current Health".to_string(),
                category: None,
                type_info: <f32>::type_info(),
                getter: Box::new(|obj| {
                    let concrete = obj.as_any().downcast_ref::<HealthComponent>().unwrap();
                    Box::new(concrete.current_health) as Box<dyn Any>
                }),
                setter: Box::new(|obj, value| {
                    let concrete = obj.as_any_mut().downcast_mut::<HealthComponent>().unwrap();
                    if let Some(v) = value.downcast_ref::<f32>() {
                        concrete.current_health = *v;
                    }
                }),
            }
        ]
    }
}
```

The editor can now display and edit properties without knowing the concrete type at compile time!

## Performance Characteristics

Understanding the costs helps you use reflection appropriately:

| Operation | Cost | Notes |
|-----------|------|-------|
| `T::type_info()` | O(1) | Returns static reference |
| Registry lookup by TypeId | O(1) | HashMap lookup |
| Registry lookup by name | O(1) | HashMap lookup |
| Field iteration | O(n) fields | Iterate static slice |
| Property getter | O(1) + downcast | Closure call + type check |
| Property setter | O(1) + downcast | Closure call + type check |
| Serialization | O(n) fields | Visits each field once |
| Deserialization | O(n) fields | Constructs value from fields |

The key insight: **metadata access is essentially free** (just pointer dereferences), but **value operations involve dynamic dispatch** (function pointer calls, downcast checks).

## Thread Safety

All the core types are thread-safe:

- `RuntimeTypeInfo`: Immutable `'static` data, inherently thread-safe
- `RUNTIME_TYPE_REGISTRY`: Uses internal synchronization (though it's populated at startup before threads exist)
- `PropertyMetadata`: Closures are `Send + Sync`
- `Box<dyn Any + Send + Sync>`: Explicitly requires thread-safe contents

You can safely query type information from any thread, and pass property values between threads.

## Limitations by Design

Some things are impossible or impractical:

### No Mutable Type Definitions

Once a type is registered, its `RuntimeTypeInfo` cannot change. The metadata is immutable `'static` data. If you need mutable types, use the dynamic type composition system (covered in [Dynamic Types](04-dynamic-types.md)).

### No Function/Method Reflection

The system captures data layout, not behavior. You can't enumerate methods or call them by name (at least not through this system—that would require a different mechanism).

### Generic Types Need Explicit Instantiation

Each concrete instantiation of a generic type needs separate reflection:

```rust
#[derive(Reflectable)]
pub struct Container<T> {
    pub value: T,
}

// Each type you use needs to be registered
type IntContainer = Container<i32>;   // Must derive separately
type FloatContainer = Container<f32>; // Must derive separately
```

There's no way to make "all `Container<T>` for any `T`" reflectable automatically.

## Next Steps

Now that you understand the core concepts:

- **[Basic Usage](03-basic-usage.md)**: Apply these concepts to practical problems
- **[Dynamic Types](04-dynamic-types.md)**: Learn about runtime type composition
- **[Advanced Usage](05-advanced-usage.md)**: Optimize and extend the system

Or dive into the implementation:

- Read `src/runtime_types.rs` to see the data structures
- Read `src/runtime_registry.rs` to see the registry implementation
- Read `pulsar_reflection_derive/src/lib.rs` to see the proc macro

Understanding these concepts makes you dangerous (in a good way)! 🦀
