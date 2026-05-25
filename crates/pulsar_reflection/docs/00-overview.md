# Pulsar Reflection System - Complete Documentation

Welcome to the complete documentation for Pulsar Engine's runtime type reflection system! This documentation will guide you through everything you need to know about using reflection in your engine development, from the basics to advanced use cases.

## What is the Reflection System?

Think of the reflection system as a bridge between compile-time and runtime. In Rust, the compiler knows everything about your types—their names, sizes, fields, and structure. But normally, once your program is compiled, that information is gone. The reflection system captures this compile-time knowledge and makes it available at runtime, allowing your code to introspect and manipulate types dynamically.

This isn't magic or runtime code generation. It's a carefully designed system that uses Rust's procedural macros to capture type information at compile time and store it as static data that your program can query later.

## Why Do We Need This?

The Pulsar Engine needs reflection for several critical features:

**Editor Integration**: When you open a component in the editor, the UI needs to know what properties exist, what types they are, and how to display them. Without reflection, you'd need to manually write UI code for every single component type—a maintenance nightmare as your game grows.

**Serialization**: Saving your game state or level data requires converting objects to a format like JSON. Reflection allows the serializer to automatically discover what fields exist and how to save them, without you writing custom serialization code for every type.

**Blueprint System**: Visual scripting needs to know what methods and properties are available on components. Reflection provides this information dynamically, so blueprints work with any component type without special handling.

**Modding and Plugins**: When a mod adds a new component type, the engine needs to understand it without recompilation. Reflection makes this possible because types register themselves automatically.

## The Two-Layer Architecture

The reflection system has two distinct layers that work together:

### 1. Compile-Time Types (Immutable Foundation)

These are your normal Rust types marked with `#[derive(Reflectable)]`. When you compile your code, the proc macro captures their metadata and generates static descriptors. This metadata includes:

- The type's unique `TypeId` from Rust's standard library
- Its name as a string (e.g., "Vec3" or "PlayerComponent")
- Memory layout information: size in bytes and alignment requirements
- Structural details: list of fields for structs, variants for enums

These descriptors are immutable `&'static` references that live for your entire program. They're incredibly cheap to work with—just pointers to static data—and completely thread-safe.

### 2. Runtime-Composed Types (Dynamic Layer)

Built on top of the compile-time foundation, this layer allows you to create new type definitions at runtime by composing existing compile-time types. Think of it like building structures out of LEGO bricks—each brick is a compile-time type, but you can arrange them in new ways at runtime.

This is crucial for:
- Data-driven design (game designers defining entity types in JSON)
- Modding systems (mods adding new types without recompiling the engine)
- Runtime schema evolution (migrating save data between game versions)

The key constraint that maintains safety: every field in a runtime-composed type must reference a registered compile-time type. You can't create a field of an unknown type.

## Primitive Feature Modules

Primitive registrations are organized into feature-gated modules:

- `prims-core` (default): `f32`, `i32`, `u64`, `bool`, `[f32; 3]`, `[f32; 4]`
- `prims-std` (default): `String`
- `prims-serde` (optional): `serde_json::Value`

The serde primitive module is intentionally not enabled by default so downstream users can opt in only when they want JSON value reflection.

## Documentation Structure

This documentation is organized to take you from beginner to expert:

**[01-getting-started.md](01-getting-started.md)**: Your first steps with reflection. Install the system, mark your types as reflectable, and see basic examples. Start here if you're new.

**[02-core-concepts.md](02-core-concepts.md)**: Deep dive into how the system actually works. Understand `RuntimeTypeInfo`, the registry, type IDs, and the relationship between compile-time and runtime.

**[03-basic-usage.md](03-basic-usage.md)**: Practical examples of common tasks like property inspection, serialization, and working with the `EngineClass` system.

**[04-dynamic-types.md](04-dynamic-types.md)**: Complete guide to the runtime type composition system. Learn how to build new types at runtime, when to use them, and their limitations.

**[05-advanced-usage.md](05-advanced-usage.md)**: Advanced patterns and techniques. Custom serializers, optimizations, integration with other systems, and debugging reflection code.

**[06-safety-best-practices.md](06-safety-best-practices.md)**: Critical reading for production code. Understand what's safe, what's dangerous, and what's impossible. Includes examples of common mistakes and how to avoid them.

**[07-migration-guide.md](07-migration-guide.md)**: Migrating from the old `PropertyType` enum system. If you're working with existing Pulsar code, this shows how the new system replaces the old patterns.

## Quick Reference

### When should I use compile-time reflection?

- ✅ Normal components and data types in your game
- ✅ Types that need to be fast (accessed in gameplay loops)
- ✅ Types where structure is known when writing code
- ✅ Anything that needs trait implementations or custom logic

### When should I use runtime-composed types?

- ✅ Mod-defined types where structure comes from external files
- ✅ Data-driven entity definitions from JSON/YAML
- ✅ Schema evolution and data migration
- ✅ Types that need to be modified after initial creation
- ❌ Performance-critical code (hot loops, update systems)
- ❌ Types that need methods or trait implementations

## Philosophy and Design Goals

The reflection system was designed with these principles:

**Zero Maintenance**: Types register themselves automatically. You never update a central enum or registry. Add a new type, derive `Reflectable`, and it just works.

**Type Safety First**: The system uses Rust's `TypeId` and `Any` trait for safe dynamic typing. Type mismatches return errors, never cause undefined behavior.

**Pay for What You Use**: If you don't use reflection on a type, you don't pay any cost. Reflection is opt-in via the `Reflectable` derive.

**Predictable Performance**: Registry lookups are O(1) hash table operations. Field access uses safe APIs (no pointer arithmetic). Performance characteristics are clear and documented.

**Extensibility**: Plugins can define custom types that work seamlessly with engine systems. No special handling needed.

**Clear Limitations**: We document what the system can't do, not just what it can. Understanding the boundaries helps you make good design decisions.

## Getting Help

If you encounter issues or have questions:

1. **Check the relevant documentation section**: Most common questions are answered in depth
2. **Look at the examples**: The `examples/` directory has working code for common scenarios
3. **Read the error messages**: Runtime type errors include detailed information about what went wrong
4. **Review the tests**: The test suite in each module shows correct usage patterns

## Next Steps

Ready to get started? Head to **[Getting Started](01-getting-started.md)** to write your first reflectable type. If you want to understand the system deeply first, jump to **[Core Concepts](02-core-concepts.md)** for the theoretical foundation.

Welcome to type-safe runtime reflection in Rust! 🦀
