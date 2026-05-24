# Dynamic Type Composition - Complete Guide

This guide covers the runtime type composition system—one of the most powerful features of the Pulsar reflection system. You'll learn how to build new types at runtime by composing existing compile-time types, enabling modding systems, data-driven design, and runtime schema evolution.

## The Problem Dynamic Types Solve

Imagine you're building a mod system for your game. A mod author wants to add a new item type—let's say an "Enchanted Sword" with properties like `damage`, `enchantment_level`, and `glow_color`. In traditional engines, this would require either:

1. **Recompiling the entire engine** with the new type (defeats the purpose of modding)
2. **Using a key-value store** like `HashMap<String, Value>` (loses type safety and is error-prone)
3. **Hardcoding all possible item types** (inflexible and unmaintainable)

Dynamic types give you a fourth option: **compose a new type at runtime from existing compile-time building blocks**. The mod defines the structure (which fields exist), and the engine provides the field types (f32, String, Color, etc.). Type safety is maintained because all field types must be registered compile-time types.

## The Two-Layer Mental Model

Think of the type system as having two layers:

### Foundation Layer: Compile-Time Types

These are your normal Rust types marked with `#[derive(Reflectable)]`:

```rust
#[derive(Reflectable, Clone)]
pub struct Vec3 { pub x: f32, pub y: f32, pub z: f32 }

#[derive(Reflectable, Clone)]
pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }
```

These are **immutable** and **static**. They're like the elements in the periodic table—fundamental building blocks that can't be changed but can be combined.

### Composition Layer: Runtime-Composed Types

Built on top of the foundation, these are new type definitions created at runtime:

```rust
let enchanted_sword = DynamicTypeBuilder::new("EnchantedSword")
    .add_field("damage", <f32>::type_info())
    .add_field("glow_color", <Color>::type_info())
    .add_field("enchantment_level", <i32>::type_info())
    .build();
```

This creates a **new type** at runtime, but all its fields reference **existing compile-time types**. The structure is mutable (you can create variations), but the field types are fixed (they must be registered types).

## Creating Your First Dynamic Type

Let's walk through a complete example—creating a custom material type for a modding system.

### Step 1: Identify Available Field Types

First, check what compile-time types are available to use as fields:

```rust
use pulsar_reflection::RUNTIME_TYPE_REGISTRY;

// List all registered types
for type_name in RUNTIME_TYPE_REGISTRY.type_names() {
    println!("Available type: {}", type_name);
}
```

Built-in types include: `f32`, `i32`, `u64`, `bool`, `String`, `[f32; 3]` (Vec3), `[f32; 4]` (Color).

### Step 2: Build the Type Definition

```rust
use pulsar_reflection::{DynamicTypeBuilder, DYNAMIC_TYPE_REGISTRY};

// Get type info for the fields we'll use
let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
let color_info = RUNTIME_TYPE_REGISTRY.get::<[f32; 4]>().unwrap();
let string_info = RUNTIME_TYPE_REGISTRY.get::<String>().unwrap();

// Build a new material type
let material_type = DynamicTypeBuilder::new("CustomWoodMaterial")
    .add_field("albedo", color_info)
    .add_field("roughness", f32_info)
    .add_field("metallic", f32_info)
    .add_field("texture_path", string_info)
    .build();

println!("Created type: {}", material_type.name);
println!("Size: {} bytes", material_type.total_size);
println!("Fields: {}", material_type.fields.len());
```

### Step 3: Register the Type

```rust
// Register it globally so other systems can find it
let type_uuid = DYNAMIC_TYPE_REGISTRY.register(material_type.clone());

println!("Registered with UUID: {}", type_uuid);

// Later, retrieve it by UUID
let retrieved = DYNAMIC_TYPE_REGISTRY.get(&type_uuid).unwrap();

// Or by name
let by_name = DYNAMIC_TYPE_REGISTRY.get_by_name("CustomWoodMaterial").unwrap();
```

### Step 4: Create Instances

```rust
use pulsar_reflection::DynamicValue;

let mut wood_material = DynamicValue::new(material_type);

// Set field values with type checking
let albedo: [f32; 4] = [0.6, 0.4, 0.2, 1.0];
wood_material.set_field("albedo", Box::new(albedo)).unwrap();

wood_material.set_field("roughness", Box::new(0.8f32)).unwrap();
wood_material.set_field("metallic", Box::new(0.0f32)).unwrap();

wood_material.set_field("texture_path",
    Box::new("textures/oak.png".to_string())
).unwrap();
```

### Step 5: Read Values Back

```rust
// Get with automatic downcasting
let roughness = wood_material.get_field_typed::<f32>("roughness").unwrap();
println!("Roughness: {}", roughness);

// Get as type-erased reference
if let Some(any_value) = wood_material.get_field("albedo") {
    if let Some(color) = any_value.downcast_ref::<[f32; 4]>() {
        println!("Albedo: {:?}", color);
    }
}
```

## Type Safety Guarantees

Dynamic types maintain type safety through several mechanisms:

### 1. Field Type Validation

When you set a field, the system checks that the value's `TypeId` matches the field's declared type:

```rust
// ✅ This works - correct type
wood_material.set_field("roughness", Box::new(0.8f32)).unwrap();

// ❌ This fails - type mismatch
let result = wood_material.set_field("roughness", Box::new(42i32));
match result {
    Err(e) => println!("Type error: {}", e),
    _ => unreachable!(),
}
// Output: Type error: Type mismatch for field 'roughness': expected 'f32', got type_id ...
```

### 2. Field Existence Checking

```rust
// ❌ This fails - field doesn't exist
let result = wood_material.set_field("nonexistent", Box::new(1.0f32));
match result {
    Err(e) => println!("{}", e),
    _ => unreachable!(),
}
// Output: Field 'nonexistent' not found in type 'CustomWoodMaterial'
```

### 3. Compile-Time Type Grounding

You can't create a field with an arbitrary unknown type:

```rust
// ✅ This compiles - f32_info is &'static RuntimeTypeInfo
builder.add_field("value", f32_info);

// ❌ This doesn't compile - can't make up a type
// builder.add_field("value", some_random_pointer);
```

The type signature requires `&'static RuntimeTypeInfo`, which can only come from registered types. This enforces that all fields are grounded in compile-time types.

## Use Case 1: Data-Driven Entity Definitions

Game designers often want to define entity types in JSON or YAML files rather than code. Here's how dynamic types enable this:

### The JSON Definition

```json
{
    "name": "Goblin",
    "fields": [
        {"name": "health", "type": "f32"},
        {"name": "max_health", "type": "f32"},
        {"name": "attack_damage", "type": "f32"},
        {"name": "movement_speed", "type": "f32"},
        {"name": "is_hostile", "type": "bool"}
    ]
}
```

### Loading the Definition

```rust
fn load_entity_type(json_str: &str) -> Arc<DynamicTypeInfo> {
    let definition: serde_json::Value = serde_json::from_str(json_str).unwrap();

    let mut builder = DynamicTypeBuilder::new(
        definition["name"].as_str().unwrap()
    );

    for field in definition["fields"].as_array().unwrap() {
        let field_name = field["name"].as_str().unwrap();
        let field_type_name = field["type"].as_str().unwrap();

        // Look up the compile-time type
        let type_info = RUNTIME_TYPE_REGISTRY
            .get_by_name(field_type_name)
            .expect(&format!("Unknown type: {}", field_type_name));

        builder = builder.add_field(field_name, type_info);
    }

    builder.build()
}

let goblin_type = load_entity_type(goblin_json);
DYNAMIC_TYPE_REGISTRY.register(goblin_type);
```

Now designers can add new entity types by editing JSON files, no programming required!

### Spawning Entities

```rust
fn spawn_goblin(type_info: Arc<DynamicTypeInfo>) -> DynamicValue {
    let mut goblin = DynamicValue::new(type_info);

    goblin.set_field("health", Box::new(50.0f32)).unwrap();
    goblin.set_field("max_health", Box::new(50.0f32)).unwrap();
    goblin.set_field("attack_damage", Box::new(12.0f32)).unwrap();
    goblin.set_field("movement_speed", Box::new(3.5f32)).unwrap();
    goblin.set_field("is_hostile", Box::new(true)).unwrap();

    goblin
}
```

## Use Case 2: Runtime Schema Evolution

When you release a game update that adds new properties to a component, you need to migrate old save data. Dynamic types make this straightforward:

### Version 1: Initial Schema

```rust
let stats_v1 = DynamicTypeBuilder::new("PlayerStats")
    .add_field("health", <f32>::type_info())
    .add_field("mana", <f32>::type_info())
    .build();

// Old save file has this structure
let mut old_save = DynamicValue::new(stats_v1);
old_save.set_field("health", Box::new(100.0f32)).unwrap();
old_save.set_field("mana", Box::new(50.0f32)).unwrap();
```

### Version 2: Extended Schema

```rust
let stats_v2 = DynamicTypeBuilder::new("PlayerStats")
    .add_field("health", <f32>::type_info())
    .add_field("mana", <f32>::type_info())
    .add_field("stamina", <f32>::type_info())  // New!
    .add_field("shield", <f32>::type_info())   // New!
    .build();
```

### Migration Function

```rust
fn migrate_v1_to_v2(old: &DynamicValue, new_type: Arc<DynamicTypeInfo>) -> DynamicValue {
    let mut new_save = DynamicValue::new(new_type);

    // Copy existing fields
    if let Ok(health) = old.get_field_typed::<f32>("health") {
        new_save.set_field("health", Box::new(*health)).unwrap();
    }
    if let Ok(mana) = old.get_field_typed::<f32>("mana") {
        new_save.set_field("mana", Box::new(*mana)).unwrap();
    }

    // Initialize new fields with defaults
    new_save.set_field("stamina", Box::new(100.0f32)).unwrap();
    new_save.set_field("shield", Box::new(0.0f32)).unwrap();

    new_save
}

let migrated = migrate_v1_to_v2(&old_save, stats_v2);
```

Players can seamlessly load old saves in the new version!

## Use Case 3: Modding System

Here's a complete example of how a mod might add a custom item type:

### Mod Definition File (`enchanted_sword.toml`)

```toml
[item]
name = "EnchantedFireSword"
display_name = "Enchanted Fire Sword"

[[fields]]
name = "base_damage"
type = "f32"
default = 45.0

[[fields]]
name = "fire_damage"
type = "f32"
default = 20.0

[[fields]]
name = "glow_color"
type = "[f32; 4]"
default = [1.0, 0.3, 0.0, 1.0]

[[fields]]
name = "enchantment_name"
type = "String"
default = "Flames of Destruction"
```

### Mod Loader

```rust
fn load_mod_item(toml_path: &str) -> Result<Arc<DynamicTypeInfo>, String> {
    let content = std::fs::read_to_string(toml_path)?;
    let definition: toml::Value = toml::from_str(&content)?;

    let name = definition["item"]["name"].as_str().unwrap();
    let mut builder = DynamicTypeBuilder::new(name);

    for field in definition["fields"].as_array().unwrap() {
        let field_name = field["name"].as_str().unwrap();
        let type_name = field["type"].as_str().unwrap();

        let type_info = RUNTIME_TYPE_REGISTRY.get_by_name(type_name)
            .ok_or_else(|| format!("Unknown type: {}", type_name))?;

        builder = builder.add_field(field_name, type_info);
    }

    let item_type = builder.build();
    Ok(item_type)
}

// Load all mods
let sword_type = load_mod_item("mods/enchanted_sword/enchanted_sword.toml").unwrap();
let sword_uuid = DYNAMIC_TYPE_REGISTRY.register(sword_type);
println!("Loaded mod item: {}", sword_uuid);
```

The game engine can now work with the mod's item type without any special handling!

## Memory Layout and Performance

When you build a dynamic type, the system calculates its memory layout using the same rules as the Rust compiler:

```rust
let type_info = DynamicTypeBuilder::new("Example")
    .add_field("a", <f32>::type_info())  // 4 bytes, align 4
    .add_field("b", <i32>::type_info())  // 4 bytes, align 4
    .add_field("c", <bool>::type_info()) // 1 byte, align 1
    .build();

println!("Size: {}", type_info.total_size);   // 12 bytes (with padding)
println!("Align: {}", type_info.total_align); // 4 bytes

for field in &type_info.fields {
    println!("{} at offset {}", field.name, field.offset);
}
```

**Output:**
```
Size: 12 bytes
Align: 4 bytes
a at offset 0
b at offset 4
c at offset 8
```

The `bool` field is padded to maintain alignment. This layout matches what Rust would generate for an equivalent struct.

### Performance Characteristics

| Operation | Cost | Notes |
|-----------|------|-------|
| Type creation | O(n) fields | Only done once at load time |
| Type lookup | O(1) | Hash table lookup by UUID/name |
| Field access | O(1) + downcast | HashMap lookup + type check |
| Memory overhead | `~48 bytes + fields` | Type metadata storage |

Dynamic values are **not** suitable for hot paths (gameplay update loops). Use them for:
- ✅ Asset loading and initialization
- ✅ Editor UI and inspection
- ✅ Serialization and save files
- ✅ Mod-defined content
- ❌ Per-frame entity updates
- ❌ Physics calculations
- ❌ Render loops

## Limitations and Constraints

Understanding what dynamic types **cannot** do is crucial for effective use:

### 1. No Methods or Behavior

Dynamic types are pure data. They cannot have:
- Methods or associated functions
- Trait implementations
- Custom Drop logic
- Operator overloads

```rust
// ✅ You can do this
let value = dynamic_value.get_field_typed::<f32>("damage").unwrap();
let new_damage = value * 1.5;
dynamic_value.set_field("damage", Box::new(new_damage)).unwrap();

// ❌ You cannot do this
// dynamic_value.apply_damage(enemy);  // No methods!
```

If you need behavior, wrap the dynamic value:

```rust
pub struct DynamicEnemy {
    data: DynamicValue,
}

impl DynamicEnemy {
    pub fn take_damage(&mut self, amount: f32) {
        let health = self.data.get_field_typed::<f32>("health").unwrap();
        let new_health = (health - amount).max(0.0);
        self.data.set_field("health", Box::new(new_health)).unwrap();
    }
}
```

### 2. No Nested Dynamic Types

You cannot create a field whose type is another dynamic type:

```rust
let inner_type = DynamicTypeBuilder::new("Inner")
    .add_field("value", <f32>::type_info())
    .build();

// ❌ This won't work - inner_type is not a &'static RuntimeTypeInfo
// let outer_type = DynamicTypeBuilder::new("Outer")
//     .add_field("inner", ???);  // Can't reference inner_type
```

**Workaround**: Serialize nested structures to JSON and store as String fields:

```rust
let outer_type = DynamicTypeBuilder::new("Outer")
    .add_field("inner_json", <String>::type_info())
    .build();

// Serialize the inner value to JSON, store it
let inner_json = serialize_dynamic_value(&inner_value);
outer.set_field("inner_json", Box::new(inner_json)).unwrap();
```

### 3. No Generic Type Parameters

Dynamic types cannot be generic:

```rust
// ❌ This concept doesn't exist
// let vec_type = DynamicTypeBuilder::new_generic::<T>("Vec<T>")
//     .add_field("data", ...);
```

Each instantiation must be a concrete type. If you need collections, use `Vec<T>` as a field type after ensuring `Vec<T>` is registered as a compile-time type.

### 4. Not FFI-Safe

Dynamic types cannot be exposed to C/C++ code:

```rust
// ❌ Won't work for FFI
#[repr(C)]
pub struct NotFfiSafe {
    dynamic_data: DynamicValue,
}
```

Dynamic types use Rust-specific mechanisms (TypeId, trait objects) that don't translate to C ABIs.

## Best Practices

### DO Use Dynamic Types For:

✅ **Mod-defined content**: Let modders add new item types, spells, abilities
✅ **Designer-defined data**: Game designers create entity types in JSON/YAML
✅ **Schema evolution**: Migrate save data between game versions
✅ **Prototyping**: Quickly test new data structures without recompiling
✅ **Tooling and editors**: Generic property inspectors and editors

### DON'T Use Dynamic Types For:

❌ **Core gameplay types**: Use compile-time types for performance
❌ **Types needing custom logic**: Methods, traits, Drop
❌ **Hot path calculations**: Per-frame updates, physics, rendering
❌ **Binary protocols**: Use schema-based serialization
❌ **FFI boundaries**: Stick to repr(C) types

### Validation and Constraints

Dynamic types don't enforce business logic constraints. Always validate after setting fields:

```rust
fn create_player(health: f32) -> Result<DynamicValue, String> {
    if health <= 0.0 || health > 1000.0 {
        return Err("Health must be between 0 and 1000".to_string());
    }

    let player_type = DYNAMIC_TYPE_REGISTRY
        .get_by_name("Player")
        .ok_or("Player type not found")?;

    let mut player = DynamicValue::new(player_type);
    player.set_field("health", Box::new(health))?;

    Ok(player)
}
```

## Debugging Dynamic Types

When things go wrong, here are debugging strategies:

### Print Type Information

```rust
fn debug_type(type_info: &DynamicTypeInfo) {
    println!("Type: {}", type_info.name);
    println!("UUID: {:?}", type_info.type_tag);
    println!("Size: {} bytes", type_info.total_size);
    println!("Alignment: {} bytes", type_info.total_align);
    println!("\nFields:");
    for field in &type_info.fields {
        println!("  {} : {} (offset: {}, size: {})",
            field.name,
            field.base_type.type_name,
            field.offset,
            field.base_type.size
        );
    }
}
```

### Print Value Contents

```rust
fn debug_value(value: &DynamicValue) {
    println!("Value of type: {}", value.type_info.name);
    println!("\nSet fields:");
    for field_name in value.field_names() {
        println!("  {} = <value>", field_name);
    }
}
```

### Verify Type Compatibility

```rust
fn can_set_field(value: &DynamicValue, field_name: &str, type_name: &str) -> bool {
    if let Some(field) = value.type_info.get_field(field_name) {
        field.base_type.type_name == type_name
    } else {
        false
    }
}

if !can_set_field(&player, "health", "f32") {
    println!("ERROR: Cannot set health field with this type");
}
```

## Integration with EngineClass

Dynamic types work alongside compile-time EngineClass components:

```rust
// Compile-time component
#[derive(EngineClass, Reflectable, Clone)]
pub struct CoreComponent {
    #[property]
    pub id: u64,

    #[property]
    pub name: String,
}

// Runtime-composed component
let custom_component = DynamicTypeBuilder::new("ModComponent")
    .add_field("mod_data", <String>::type_info())
    .build();

// Both can be inspected in the editor!
```

The editor can show properties from both compile-time and runtime types using the same property inspection UI.

## Conclusion

Dynamic type composition is a powerful feature that enables modding, data-driven design, and schema evolution while maintaining type safety. The key insight is that you're not creating types from nothing—you're composing new structures from existing, validated building blocks.

Remember:
- All field types must be registered compile-time types
- Dynamic types are data-only, no methods or traits
- Use them for configuration and content, not gameplay logic
- Validate business rules at the application level
- Performance is fine for loading and editing, not for hot paths

For complete working examples, see `examples/dynamic_types.rs` in the repository.

## Next Steps

- **[Advanced Usage](05-advanced-usage.md)**: Custom serializers, performance optimization
- **[Safety and Best Practices](06-safety-best-practices.md)**: Avoiding common pitfalls
- **[Examples](../examples/)**: See complete working code

Happy composing! 🎭
