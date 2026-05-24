# Runtime Type Reflection System - Developer Guide

## Introduction

The Pulsar Engine runtime type reflection system provides compile-time captured type metadata that's available at runtime without enum pattern matching. This system was designed to replace the old `PropertyType` enum, which required manual maintenance every time a new type was added to the engine. Instead of maintaining hardcoded enum variants and updating pattern matches across dozens of files, types now register themselves automatically using procedural macros and are looked up via their `TypeId` at runtime.

This guide will walk you through how to use the reflection system safely, when to be careful with unsafe operations, and most importantly, what this system **cannot** do so you don't waste time trying to use it incorrectly.

## How It Works

When you derive `Reflectable` on a type, the proc macro analyzes your struct or enum at compile time and generates a static `RuntimeTypeInfo` descriptor containing the type's `TypeId`, size, alignment, name, and structural information. This descriptor is automatically registered with the global `RUNTIME_TYPE_REGISTRY` via the `inventory` crate, which collects these registrations at link time. At runtime, you can query the registry using either the `TypeId` or type name to retrieve complete metadata about any registered type without knowing its concrete type at compile time.

The key insight is that this approach eliminates the disconnect between compile-time type information (which Rust already has) and runtime type information (which previously required manual enum maintenance). Now these stay in sync automatically because they're generated from the same source: your type definitions.

## Basic Safe Usage

For most engine development, you'll interact with the reflection system through the `Reflectable` trait. Here's how to use it safely with components and properties:

```rust
use pulsar_reflection::Reflectable;

// Deriving Reflectable automatically registers your type
#[derive(Reflectable, Clone, Debug)]
pub struct TransformComponent {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

// Later, you can query type information at runtime
fn inspect_component_type<T: Reflectable>() {
    let type_info = T::type_info();
    println!("Type: {}", type_info.type_name);
    println!("Size: {} bytes", type_info.size);
    println!("Alignment: {} bytes", type_info.align);

    // Access struct fields if it's a struct
    if let Some(fields) = type_info.fields() {
        for field in fields {
            println!("  Field '{}': {}", field.name, field.type_info.type_name);
        }
    }
}
```

This is completely safe because you're only reading metadata that was captured at compile time. The `RuntimeTypeInfo` structure is immutable after creation, and all the pointers it contains are `&'static` references that live for the entire program duration.

When working with engine classes and properties, the `EngineClass` derive macro automatically integrates with the reflection system. You don't need to manually specify property types anymore—the macro queries the `Reflectable` implementation for each field:

```rust
use engine_class_derive::EngineClass;
use pulsar_reflection::Reflectable;

#[derive(EngineClass, Reflectable, Default, Clone)]
pub struct PhysicsComponent {
    #[property]
    pub mass: f32,

    #[property]
    pub friction: f32,

    #[property]
    pub velocity: Vec3,
}

// The macro generates PropertyMetadata that references RuntimeTypeInfo
// No manual type specification needed!
```

The generated property metadata includes type-erased getters and setters that return and accept `Box<dyn Any>`. While this involves dynamic dispatch, it's safe because the getters/setters perform runtime type checks using `downcast_ref` and `downcast_mut`, which will return `None` if the type doesn't match rather than causing undefined behavior.

## Working with Type-Erased Values

When you retrieve properties from engine classes, you get a `Box<dyn Any>` that you need to downcast to the concrete type. This is where you need to be more careful, though it's still memory-safe:

```rust
fn modify_property(component: &mut dyn EngineClass, prop_name: &str, new_value: f32) {
    let properties = component.get_properties();

    if let Some(prop) = properties.iter().find(|p| p.name == prop_name) {
        // Create a type-erased value
        let boxed_value: Box<dyn Any> = Box::new(new_value);

        // The setter will attempt to downcast internally
        (prop.setter)(component, boxed_value);

        // If the type doesn't match, it logs a warning but doesn't crash
    }
}
```

The important safety property here is that even though we're working with `dyn Any`, Rust's type system ensures we can only successfully downcast to the original type. Attempting to downcast to the wrong type simply returns `None`, it never causes memory corruption. The worst that can happen is a warning log message if you try to set a property with the wrong type.

> [!WARNING]
> **Type Mismatches Are Logged, Not Enforced at Compile Time**
>
> When using `Box<dyn Any>` with property setters, type mismatches are detected at runtime via `downcast_ref`, not at compile time. If you pass an `i32` to a property expecting `f32`, the setter will fail silently with a warning log. This is by design for maximum flexibility, but it means you need to ensure type correctness yourself when working with type-erased APIs.
>
> If you need compile-time type safety, work with the concrete component types directly instead of going through the `dyn EngineClass` trait object.

## Serialization and Deserialization

The reflection system includes trait-based serialization that works with any type implementing `Reflectable`. The JSON serializer and deserializer are provided out of the box, and you can implement custom serializers for binary formats or network protocols:

```rust
use pulsar_reflection::{Reflectable, JsonSerializer, JsonDeserializer};

#[derive(Reflectable, Clone)]
pub struct SaveData {
    pub player_name: String,
    pub level: i32,
    pub health: f32,
}

fn save_game(data: &SaveData) -> String {
    let mut serializer = JsonSerializer::new();
    data.serialize(&mut serializer).unwrap();
    serde_json::to_string_pretty(serializer.as_json()).unwrap()
}

fn load_game(json_str: &str) -> Result<SaveData, Box<dyn Error>> {
    let json: serde_json::Value = serde_json::from_str(json_str)?;
    let mut deserializer = JsonDeserializer::new(json);
    Ok(SaveData::deserialize(&mut deserializer)?)
}
```

This is safe because the serialization format is validated against the type's structure. If the JSON is missing a required field, deserialization returns an error rather than producing an invalid object. The type system ensures you can't accidentally deserialize to the wrong type.

However, there's an important limitation with nested types and generic containers:

> [!CAUTION]
> **Manual Type Registration Required for Complex Nested Types**
>
> The `#[derive(Reflectable)]` macro automatically registers your type and recursively generates field information for struct types. However, it requires that all field types also implement `Reflectable`. If you have a field of type `Vec<CustomType>`, both `Vec<T>` and `CustomType` must be reflectable.
>
> Currently, generic container types like `Vec<T>`, `Option<T>`, and `HashMap<K, V>` need to be registered for each concrete instantiation you use. For example, `Vec<f32>` and `Vec<String>` are different types that need separate registrations. We're working on automatic generic instantiation registration, but for now you may need to manually register commonly used generic types.

## Unsafe Usage Patterns

While the reflection system itself is built on safe Rust, there are a few scenarios where you might be tempted to use unsafe code with it. Let's be clear about what's safe and what requires extreme caution.

### Raw Pointer Dereferencing (NEVER DO THIS)

The `FieldInfo` structure includes an `offset` field that tells you where a field is located within a struct. You might be tempted to use this with raw pointer arithmetic to directly access fields:

> [!CAUTION]
> **NEVER Use Field Offsets for Direct Memory Access**
>
> ```rust
> // ❌ EXTREMELY DANGEROUS - DO NOT DO THIS!
> unsafe fn get_field_unchecked<T>(obj: *const T, field_offset: usize) -> &f32 {
>     let field_ptr = (obj as *const u8).add(field_offset) as *const f32;
>     &*field_ptr  // UNDEFINED BEHAVIOR if T doesn't have an f32 at this offset!
> }
> ```
>
> This is undefined behavior waiting to happen. Even if the offset looks correct, there are multiple ways this can fail:
> - Alignment requirements might not be met (accessing unaligned data)
> - The type at that offset might not be what you expect (reading wrong type)
> - The struct layout might change between compilations (not guaranteed stable)
> - Padding bytes might exist that you're not accounting for
> - The lifetime of the reference you create might be invalid
>
> The field offset is provided for debug inspection and tooling purposes only. Use the getters and setters provided by `PropertyMetadata` instead, which handle all of this safely.

### Downcasting Arbitrary Pointers

Another dangerous pattern is attempting to downcast raw pointers or references to trait objects without proper type checking:

> [!CAUTION]
> **NEVER Downcast Without TypeId Verification**
>
> ```rust
> // ❌ DANGEROUS - Blindly assuming type matches
> unsafe fn extract_value_unchecked(any_value: &dyn Any) -> f32 {
>     // This might work, or it might read random memory as f32
>     *(any_value as *const dyn Any as *const f32)
> }
>
> // ✅ SAFE - Always use downcast_ref
> fn extract_value_safe(any_value: &dyn Any) -> Option<f32> {
>     any_value.downcast_ref::<f32>().copied()
> }
> ```
>
> The safe version uses `downcast_ref`, which checks the `TypeId` internally before performing the cast. If the type doesn't match, you get `None` instead of reading garbage memory as the wrong type. There is no performance reason to skip this check—it's a single pointer comparison.

### Transmuting Between Types

Sometimes developers think they can use `std::mem::transmute` to convert between types if they have the same size:

> [!CAUTION]
> **NEVER Use transmute with Reflected Types**
>
> ```rust
> // ❌ CATASTROPHICALLY DANGEROUS
> fn convert_property_unchecked(value: Box<dyn Any>, target_type: &RuntimeTypeInfo) -> Box<dyn Any> {
>     if value.type_id() != target_type.type_id {
>         // "They're the same size, so transmute should work, right?"
>         unsafe {
>             let boxed_slice = Box::into_raw(value) as *mut [u8; 4];
>             let retyped = Box::from_raw(boxed_slice as *mut f32);
>             Box::new(retyped) as Box<dyn Any>
>         }
>     } else {
>         value
>     }
> }
> ```
>
> This is wrong on so many levels:
> - Same size doesn't mean compatible representation (i32 and f32 are both 4 bytes but different values)
> - Box layouts aren't guaranteed to be compatible
> - You're violating type safety guarantees
> - This will cause memory corruption, crashes, or wrong computation
>
> If you need to convert between types, deserialize to one type and reserialize to the other. Yes, it's slower. No, there's no shortcut.

## What This System Cannot Do

Understanding the limitations of the reflection system is just as important as understanding its capabilities. Here are scenarios where reflection cannot help you, and trying to use it will either fail or produce incorrect results.

### Runtime Type Creation

The reflection system does **not** allow you to create new types at runtime or dynamically modify type definitions. Everything is captured at compile time:

```rust
// ❌ THIS WILL NEVER WORK
fn create_dynamic_struct(field_names: Vec<String>) -> Box<dyn Reflectable> {
    // You cannot dynamically create new struct types at runtime!
    // Rust is statically typed - all types must exist at compile time
    panic!("Impossible!");
}
```

If you need runtime-flexible data structures, use `HashMap<String, Box<dyn Any>>` or similar dynamic containers. The reflection system describes existing types, it doesn't create new ones.

### Generic Type Inference

While the reflection system can describe generic types after they're monomorphized (turned into concrete types), it cannot perform type inference or determine what generic parameters should be:

```rust
// ❌ THIS DOESN'T WORK
fn deserialize_generic<T>(json: &str) -> T {
    // You have the type parameter T, but RuntimeTypeInfo
    // can't help you figure out what T is from just JSON
    let type_info = T::type_info();  // This requires T to be known!
    // There's no way to go backwards from JSON to determine T
}
```

You always need to know the concrete type at some point. The reflection system helps you work with that type dynamically after you know what it is, but it can't figure out the type for you.

### Cross-Language Reflection

The `RuntimeTypeInfo` is specific to Rust types and uses Rust's `TypeId`. It does not provide FFI-safe reflection or interoperability with other languages' type systems:

```rust
// ❌ WILL NOT WORK FOR C/C++ INTEROP
#[repr(C)]
#[derive(Reflectable)]  // The Reflectable implementation isn't FFI-safe!
pub struct ExportedStruct {
    pub value: f32,
}

// C code cannot use RuntimeTypeInfo or Reflectable trait
```

If you need to expose types to C/C++, you still need to write explicit FFI bindings. The reflection system is for Rust-to-Rust introspection only.

### Performance-Critical Hot Paths

While the reflection system is reasonably fast (type lookups are O(1) hash table lookups), it's still slower than using types directly. Do not use reflection in performance-critical code:

```rust
// ❌ BAD: Using reflection in a hot loop
for entity in entities.iter_mut() {
    let props = entity.get_properties();  // Allocates Vec
    for prop in props {
        let value = (prop.getter)(entity);  // Dynamic dispatch
        let new_value = transform(value);   // Downcast overhead
        (prop.setter)(entity, new_value);   // More dynamic dispatch
    }
}

// ✅ GOOD: Direct access in hot paths
for entity in entities.iter_mut() {
    entity.position += entity.velocity * delta_time;  // Direct field access
}
```

Use reflection for editor UI, serialization, debugging tools, and plugin systems. Don't use it in your gameplay update loop.

### Procedural Macro Limitations

The `#[derive(Reflectable)]` macro analyzes your code at compile time, but it has limitations:

- **Cannot analyze macro-generated code**: If another proc macro generates your struct, Reflectable might not see the final result
- **Cannot handle all type expressions**: Complex type expressions or associated types might not be fully supported
- **No conditional compilation per-field**: You can't make fields reflectable only in debug builds (it's all or nothing)

Additionally, the current implementation requires all field types to also be `Reflectable`. If you have a field whose type doesn't implement `Reflectable`, the derive will fail:

```rust
// ❌ WILL FAIL TO COMPILE
#[derive(Reflectable)]
pub struct MyStruct {
    pub data: SomeExternalType,  // If this doesn't implement Reflectable, compilation fails
}
```

You'll need to either implement `Reflectable` for `SomeExternalType` or exclude that field from reflection.