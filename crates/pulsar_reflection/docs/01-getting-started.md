# Getting Started with Reflection

This guide will walk you through your first steps with the Pulsar reflection system. By the end, you'll have created reflectable types, queried their metadata, and understood the basics of how the system works.

## Prerequisites

You should be familiar with:
- Basic Rust syntax (structs, enums, traits)
- Derive macros (`#[derive(Debug, Clone)]` etc.)
- The concept of `TypeId` from `std::any`

No prior knowledge of reflection systems is required!

## Adding Reflection to Your Project

The reflection system is part of the core `pulsar_reflection` crate. If you're working within the Pulsar Engine workspace, it's already available. Otherwise, add it to your `Cargo.toml`:

```toml
[dependencies]
pulsar_reflection = { path = "../pulsar_reflection" }
```

## Your First Reflectable Type

Let's start with the simplest possible example—a struct with a few fields:

```rust
use pulsar_reflection::Reflectable;

#[derive(Reflectable, Clone, Debug)]
pub struct PlayerPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
```

That's it! By adding `#[derive(Reflectable)]`, you've made this type introspectable at runtime. The proc macro has automatically:

1. Analyzed the struct's fields at compile time
2. Generated a static descriptor containing all type metadata
3. Registered the type in the global type registry
4. Implemented the `Reflectable` trait

All of this happens at compile time with zero runtime overhead for the registration itself.

## Accessing Type Information

Now that your type is reflectable, let's query its metadata:

```rust
use pulsar_reflection::RUNTIME_TYPE_REGISTRY;

fn main() {
    // Look up the type by its TypeId
    let type_info = RUNTIME_TYPE_REGISTRY.get::<PlayerPosition>();

    if let Some(info) = type_info {
        println!("Type name: {}", info.type_name);
        println!("Size: {} bytes", info.size);
        println!("Alignment: {} bytes", info.align);

        // Check if it's a struct
        if info.is_struct() {
            println!("This is a struct type!");

            // Iterate over fields
            if let Some(fields) = info.fields() {
                println!("\nFields:");
                for field in fields {
                    println!("  {} : {} (offset: {}, size: {})",
                        field.name,
                        field.type_info.type_name,
                        field.offset,
                        field.type_info.size
                    );
                }
            }
        }
    }
}
```

**Output:**
```
Type name: PlayerPosition
Size: 12 bytes
Alignment: 4 bytes
This is a struct type!

Fields:
  x : f32 (offset: 0, size: 4)
  y : f32 (offset: 4, size: 4)
  z : f32 (offset: 8, size: 4)
```

Notice how we got complete information about the type's structure, field names, and memory layout—all without manually writing any registration code.

## Understanding What Just Happened

When you derived `Reflectable`, the macro generated code similar to this (simplified):

```rust
// The macro generates this static descriptor
static PLAYER_POSITION_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<PlayerPosition>(),
    type_name: "PlayerPosition",
    size: std::mem::size_of::<PlayerPosition>(),
    align: std::mem::align_of::<PlayerPosition>(),
    structure: TypeStructure::Struct {
        fields: &[
            FieldInfo {
                name: "x",
                type_info: <f32>::type_info(),
                offset: 0,
            },
            // ... more fields
        ],
    },
};

// And this registration
inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &PLAYER_POSITION_TYPE_INFO,
    }
}

// And implements the trait
impl Reflectable for PlayerPosition {
    fn type_info() -> &'static RuntimeTypeInfo {
        &PLAYER_POSITION_TYPE_INFO
    }

    // ... serialization methods
}
```

The `inventory` crate collects all these registrations at link time, so when your program starts, the type is already in the registry. No manual init function needed!

## Working with Enums

Reflection also works with enums. Here's an example:

```rust
#[derive(Reflectable, Clone, Debug, PartialEq)]
pub enum MovementState {
    Idle,
    Walking,
    Running,
    Jumping,
}
```

You can query enum variants at runtime:

```rust
let type_info = RUNTIME_TYPE_REGISTRY.get::<MovementState>().unwrap();

if let Some(variants) = type_info.enum_variants() {
    println!("Enum variants:");
    for (index, variant) in variants.iter().enumerate() {
        println!("  {} = {}", index, variant);
    }
}
```

**Output:**
```
Enum variants:
  0 = Idle
  1 = Walking
  2 = Running
  3 = Jumping
```

## Requirements for Reflectable Types

Not every type can be made reflectable. Here are the requirements:

### Must Implement
- **`Clone`**: Reflection needs to be able to create copies of values
- **`Send + Sync`**: For thread safety in the type-erased APIs
- **`'static`**: No borrowed references in your type

### Field Type Requirements
Every field in your struct must also be reflectable. This works automatically for:
- Primitive types: `f32`, `i32`, `u64`, `bool`
- Standard types: `String`
- Arrays: `[f32; 3]`, `[f32; 4]`
- Other reflectable types

### What About Generics?

Generic types work, but each concrete instantiation needs to be explicitly derived:

```rust
#[derive(Reflectable, Clone)]
pub struct Container<T> {
    pub value: T,
}

// Each specific type you use must derive Reflectable
type IntContainer = Container<i32>;
type FloatContainer = Container<f32>;
```

Currently, generic types aren't automatically reflectable—you need to derive for each concrete type you use.

## A Practical Example: Component System

Let's put it all together with a realistic game component:

```rust
use pulsar_reflection::Reflectable;

#[derive(Reflectable, Clone, Debug)]
pub struct HealthComponent {
    pub current_health: f32,
    pub max_health: f32,
    pub is_invulnerable: bool,
    pub regeneration_rate: f32,
}

impl HealthComponent {
    pub fn new(max_health: f32) -> Self {
        Self {
            current_health: max_health,
            max_health,
            is_invulnerable: false,
            regeneration_rate: 0.0,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.current_health > 0.0
    }

    pub fn take_damage(&mut self, amount: f32) {
        if !self.is_invulnerable {
            self.current_health = (self.current_health - amount).max(0.0);
        }
    }
}

fn main() {
    // Your component has normal methods
    let mut health = HealthComponent::new(100.0);
    health.take_damage(25.0);
    println!("Health: {}", health.current_health);

    // But it's also reflectable!
    let type_info = RUNTIME_TYPE_REGISTRY.get::<HealthComponent>().unwrap();
    println!("\nComponent type: {}", type_info.type_name);
    println!("Number of properties: {}", type_info.fields().unwrap().len());

    // Editors can now discover properties automatically
    for field in type_info.fields().unwrap() {
        println!("  - {} ({})", field.name, field.type_info.type_name);
    }
}
```

**Output:**
```
Health: 75

Component type: HealthComponent
Number of properties: 4
  - current_health (f32)
  - max_health (f32)
  - is_invulnerable (bool)
  - regeneration_rate (f32)
```

This is the foundation of how the Pulsar editor discovers component properties without you writing any UI code!

## Serialization Basics

Every reflectable type automatically gets serialization support. Here's a quick example:

```rust
use pulsar_reflection::{JsonSerializer, JsonDeserializer};
use serde_json;

let health = HealthComponent::new(100.0);

// Serialize to JSON
let mut serializer = JsonSerializer::new();
health.serialize(&mut serializer).unwrap();
let json = serde_json::to_string_pretty(serializer.as_json()).unwrap();

println!("Serialized:\n{}", json);

// Deserialize back
let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
let mut deserializer = JsonDeserializer::new(parsed);
let restored = HealthComponent::deserialize(&mut deserializer).unwrap();

println!("\nRestored health: {}", restored.current_health);
```

The serializer uses the type's reflection metadata to automatically discover and serialize all fields. No manual `Serialize` implementation needed!

## Common Pitfalls for Beginners

### Forgetting Clone

```rust
// ❌ This won't compile
#[derive(Reflectable)]
pub struct BadComponent {
    pub data: Vec<u8>,
}

// ✅ This works
#[derive(Reflectable, Clone)]
pub struct GoodComponent {
    pub data: Vec<u8>,
}
```

### Non-Reflectable Field Types

```rust
// ❌ This fails if SomeExternalType doesn't implement Reflectable
#[derive(Reflectable, Clone)]
pub struct BadComponent {
    pub data: SomeExternalType,
}

// ✅ Either make the external type reflectable, or wrap it
#[derive(Reflectable, Clone)]
pub struct GoodComponent {
    pub data: String,  // Use a reflectable alternative
}
```

### Trying to Reflect References

```rust
// ❌ References aren't allowed
#[derive(Reflectable)]
pub struct BadComponent<'a> {
    pub name: &'a str,
}

// ✅ Use owned types
#[derive(Reflectable, Clone)]
pub struct GoodComponent {
    pub name: String,
}
```

## Next Steps

Now that you understand the basics, you're ready for more advanced topics:

- **[Core Concepts](02-core-concepts.md)**: Deep dive into how `RuntimeTypeInfo`, type IDs, and the registry work
- **[Basic Usage](03-basic-usage.md)**: Practical patterns for common tasks
- **[Dynamic Types](04-dynamic-types.md)**: Learn about runtime type composition for modding and data-driven design

Or jump straight to the **[examples directory](../examples/)** to see complete working code!

## Quick Reference

```rust
// Make a type reflectable
#[derive(Reflectable, Clone)]
pub struct MyType { /* fields */ }

// Look up by type
let info = RUNTIME_TYPE_REGISTRY.get::<MyType>().unwrap();

// Look up by name
let info = RUNTIME_TYPE_REGISTRY.get_by_name("MyType").unwrap();

// Access metadata
info.type_name    // "MyType"
info.size         // size in bytes
info.align        // alignment in bytes
info.type_id      // std::any::TypeId

// Check type kind
info.is_struct()
info.is_enum()
info.is_primitive()

// Access structure
if let Some(fields) = info.fields() { /* ... */ }
if let Some(variants) = info.enum_variants() { /* ... */ }
```

Happy reflecting! 🦀
