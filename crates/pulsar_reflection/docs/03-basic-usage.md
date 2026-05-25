# Basic Usage - Common Patterns and Recipes

This guide provides practical examples for common reflection tasks. Each section shows a complete, working pattern you can adapt to your needs.

## Table of Contents

1. [Inspecting Type Information](#inspecting-type-information)
2. [Working with Properties](#working-with-properties)
3. [Serialization and Deserialization](#serialization-and-deserialization)
4. [Type-Safe Property Access](#type-safe-property-access)
5. [Building Generic Tools](#building-generic-tools)
6. [Error Handling](#error-handling)
7. [Integration with EngineClass](#integration-with-engineclass)

## Inspecting Type Information

### Check if a Type is Registered

```rust
use pulsar_reflection::RUNTIME_TYPE_REGISTRY;
use std::any::TypeId;

fn is_type_registered<T: 'static>() -> bool {
    RUNTIME_TYPE_REGISTRY.get::<T>().is_some()
}

// Usage
if is_type_registered::<f32>() {
    println!("f32 is registered!");
}
```

### List All Registered Types

```rust
fn list_all_types() {
    let registry = &*RUNTIME_TYPE_REGISTRY;
    let type_names = registry.type_names();

    println!("Registered types:");
    for name in type_names {
        println!("  - {}", name);
    }
}
```

### Get Detailed Type Information

```rust
fn print_type_details<T: 'static>() {
    let registry = &*RUNTIME_TYPE_REGISTRY;

    if let Some(type_info) = registry.get::<T>() {
        println!("Type: {}", type_info.type_name);
        println!("Size: {} bytes", type_info.size);
        println!("Alignment: {} bytes", type_info.align);
        println!("TypeId: {:?}", type_info.type_id);

        // Check what kind of type it is
        if type_info.is_primitive() {
            println!("Kind: Primitive");
        } else if type_info.is_struct() {
            println!("Kind: Struct");
            if let Some(fields) = type_info.fields() {
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
        } else if type_info.is_enum() {
            println!("Kind: Enum");
            if let Some(variants) = type_info.enum_variants() {
                println!("\nVariants:");
                for (i, variant) in variants.iter().enumerate() {
                    println!("  {} = {}", i, variant);
                }
            }
        }
    } else {
        println!("Type not registered");
    }
}
```

### Compare Types

```rust
fn same_type(a: &RuntimeTypeInfo, b: &RuntimeTypeInfo) -> bool {
    a.type_id == b.type_id
}

// Usage
let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
let i32_info = RUNTIME_TYPE_REGISTRY.get::<i32>().unwrap();

assert!(!same_type(f32_info, i32_info));
```

## Working with Properties

### Iterate Over Component Properties

```rust
use pulsar_reflection::EngineClass;

fn list_component_properties(component: &dyn EngineClass) {
    let properties = component.get_properties();

    println!("Component: {}", component.class_name());
    println!("Properties:");

    for prop in properties {
        println!("  {} : {} ({})",
            prop.name,
            prop.type_info.type_name,
            prop.display_name
        );
    }
}
```

### Get a Property Value

```rust
fn get_property_value(
    component: &dyn EngineClass,
    property_name: &str
) -> Option<Box<dyn Any>> {
    let properties = component.get_properties();

    let prop = properties.iter()
        .find(|p| p.name == property_name)?;

    Some((prop.getter)(component))
}

// Usage
let health_component = HealthComponent { current_health: 100.0, max_health: 100.0 };
let value = get_property_value(&health_component as &dyn EngineClass, "current_health");

if let Some(boxed) = value {
    if let Some(health) = boxed.downcast_ref::<f32>() {
        println!("Current health: {}", health);
    }
}
```

### Set a Property Value

```rust
fn set_property_value(
    component: &mut dyn EngineClass,
    property_name: &str,
    value: Box<dyn Any>
) -> Result<(), String> {
    let properties = component.get_properties();

    let prop = properties.iter()
        .find(|p| p.name == property_name)
        .ok_or_else(|| format!("Property '{}' not found", property_name))?;

    // Type check
    if value.as_ref().type_id() != prop.type_info.type_id {
        return Err(format!(
            "Type mismatch: expected '{}', got '{:?}'",
            prop.type_info.type_name,
            value.as_ref().type_id()
        ));
    }

    (prop.setter)(component, value);
    Ok(())
}

// Usage
let mut health = HealthComponent { current_health: 100.0, max_health: 100.0 };
set_property_value(
    &mut health as &mut dyn EngineClass,
    "current_health",
    Box::new(75.0f32)
).unwrap();
```

### Find Properties by Type

```rust
fn find_properties_of_type(
    component: &dyn EngineClass,
    type_name: &str
) -> Vec<String> {
    component.get_properties()
        .into_iter()
        .filter(|p| p.type_info.type_name == type_name)
        .map(|p| p.name.to_string())
        .collect()
}

// Usage: Find all f32 properties
let f32_properties = find_properties_of_type(&health, "f32");
println!("f32 properties: {:?}", f32_properties);
```

## Serialization and Deserialization

### Serialize to JSON

```rust
use pulsar_reflection::{Reflectable, JsonSerializer};
use serde_json;

fn to_json<T: Reflectable>(value: &T) -> Result<String, Box<dyn std::error::Error>> {
    let mut serializer = JsonSerializer::new();
    value.serialize(&mut serializer)?;

    let json = serde_json::to_string_pretty(serializer.as_json())?;
    Ok(json)
}

// Usage
#[derive(Reflectable, Clone)]
struct SaveData {
    player_name: String,
    level: i32,
    score: f32,
}

let save = SaveData {
    player_name: "Hero".to_string(),
    level: 10,
    score: 12345.0,
};

let json = to_json(&save).unwrap();
println!("{}", json);
```

### Deserialize from JSON

```rust
use pulsar_reflection::JsonDeserializer;

fn from_json<T: Reflectable>(json_str: &str) -> Result<T, Box<dyn std::error::Error>> {
    let json: serde_json::Value = serde_json::from_str(json_str)?;
    let mut deserializer = JsonDeserializer::new(json);
    Ok(T::deserialize(&mut deserializer)?)
}

// Usage
let json = r#"{
    "player_name": "Hero",
    "level": 10,
    "score": 12345.0
}"#;

let save: SaveData = from_json(json).unwrap();
println!("Loaded: {} at level {}", save.player_name, save.level);
```

### Serialize Components Generically

```rust
fn serialize_component(component: &dyn EngineClass) -> Result<String, Box<dyn std::error::Error>> {
    let mut json_obj = serde_json::Map::new();

    // Add class name
    json_obj.insert("class".to_string(), serde_json::json!(component.class_name()));

    // Add properties
    let properties = component.get_properties();
    for prop in properties {
        let value = (prop.getter)(component);

        // Convert to JSON (simplified - would need proper type dispatch)
        if let Some(f) = value.downcast_ref::<f32>() {
            json_obj.insert(prop.name.to_string(), serde_json::json!(f));
        } else if let Some(i) = value.downcast_ref::<i32>() {
            json_obj.insert(prop.name.to_string(), serde_json::json!(i));
        } else if let Some(b) = value.downcast_ref::<bool>() {
            json_obj.insert(prop.name.to_string(), serde_json::json!(b));
        } else if let Some(s) = value.downcast_ref::<String>() {
            json_obj.insert(prop.name.to_string(), serde_json::json!(s));
        }
    }

    Ok(serde_json::to_string_pretty(&json_obj)?)
}
```

### Clone Values Using Reflection

```rust
fn clone_reflectable(value: &dyn Reflectable) -> Box<dyn Any> {
    value.clone_any()
}

// Usage
let original = HealthComponent { current_health: 100.0, max_health: 150.0 };
let cloned_any = clone_reflectable(&original);

// Downcast back
if let Some(cloned) = cloned_any.downcast_ref::<HealthComponent>() {
    println!("Cloned health: {}", cloned.current_health);
}
```

## Type-Safe Property Access

### Typed Property Wrapper

```rust
pub struct TypedProperty<'a, T: 'static> {
    component: &'a dyn EngineClass,
    property_name: &'static str,
    _phantom: std::marker::PhantomData<T>,
}

impl<'a, T: 'static + Clone> TypedProperty<'a, T> {
    pub fn new(component: &'a dyn EngineClass, property_name: &'static str) -> Self {
        Self {
            component,
            property_name,
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn get(&self) -> Result<T, String> {
        let properties = self.component.get_properties();
        let prop = properties.iter()
            .find(|p| p.name == self.property_name)
            .ok_or_else(|| format!("Property '{}' not found", self.property_name))?;

        let value = (prop.getter)(self.component);
        value.downcast_ref::<T>()
            .cloned()
            .ok_or_else(|| format!("Type mismatch for property '{}'", self.property_name))
    }
}

// Usage
let health = HealthComponent { current_health: 100.0, max_health: 150.0 };
let property = TypedProperty::<f32>::new(&health as &dyn EngineClass, "current_health");

match property.get() {
    Ok(value) => println!("Health: {}", value),
    Err(e) => eprintln!("Error: {}", e),
}
```

## Building Generic Tools

### Generic Property Inspector

```rust
pub struct PropertyInspector {
    component: Box<dyn EngineClass>,
}

impl PropertyInspector {
    pub fn new(component: Box<dyn EngineClass>) -> Self {
        Self { component }
    }

    pub fn list_properties(&self) {
        println!("=== {} Properties ===", self.component.class_name());

        for prop in self.component.get_properties() {
            let value = (prop.getter)(self.component.as_ref());
            let value_str = self.format_value(&value, prop.type_info);

            println!("{} ({}): {}",
                prop.display_name,
                prop.type_info.type_name,
                value_str
            );
        }
    }

    fn format_value(&self, value: &Box<dyn Any>, type_info: &RuntimeTypeInfo) -> String {
        // Try common types
        if let Some(f) = value.downcast_ref::<f32>() {
            format!("{:.2}", f)
        } else if let Some(i) = value.downcast_ref::<i32>() {
            format!("{}", i)
        } else if let Some(b) = value.downcast_ref::<bool>() {
            format!("{}", b)
        } else if let Some(s) = value.downcast_ref::<String>() {
            format!("\"{}\"", s)
        } else {
            format!("<{}>", type_info.type_name)
        }
    }

    pub fn modify_property(&mut self, name: &str, new_value: Box<dyn Any>) -> Result<(), String> {
        let properties = self.component.get_properties();
        let prop = properties.iter()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("Property '{}' not found", name))?;

        if new_value.as_ref().type_id() != prop.type_info.type_id {
            return Err(format!("Type mismatch for '{}'", name));
        }

        (prop.setter)(self.component.as_mut(), new_value);
        Ok(())
    }
}

// Usage
let health = HealthComponent { current_health: 100.0, max_health: 150.0 };
let mut inspector = PropertyInspector::new(Box::new(health));

inspector.list_properties();
inspector.modify_property("current_health", Box::new(75.0f32)).unwrap();
inspector.list_properties();
```

### Generic Validator

```rust
pub trait PropertyValidator {
    fn validate(&self, value: &dyn Any, type_info: &RuntimeTypeInfo) -> Result<(), String>;
}

pub struct RangeValidator {
    pub min: f32,
    pub max: f32,
}

impl PropertyValidator for RangeValidator {
    fn validate(&self, value: &dyn Any, type_info: &RuntimeTypeInfo) -> Result<(), String> {
        if type_info.type_id != TypeId::of::<f32>() {
            return Ok(()); // Only validate f32
        }

        if let Some(f) = value.downcast_ref::<f32>() {
            if *f < self.min || *f > self.max {
                return Err(format!("Value {} out of range [{}, {}]", f, self.min, self.max));
            }
        }

        Ok(())
    }
}

// Usage
let validator = RangeValidator { min: 0.0, max: 100.0 };
let value: Box<dyn Any> = Box::new(150.0f32);
let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

match validator.validate(&value, f32_info) {
    Ok(()) => println!("Valid"),
    Err(e) => println!("Invalid: {}", e),
}
```

### Property Comparison

```rust
fn properties_equal(
    a: &dyn EngineClass,
    b: &dyn EngineClass,
    property_name: &str
) -> Result<bool, String> {
    if a.class_name() != b.class_name() {
        return Err("Components are different types".to_string());
    }

    let props_a = a.get_properties();
    let props_b = b.get_properties();

    let prop_a = props_a.iter()
        .find(|p| p.name == property_name)
        .ok_or_else(|| format!("Property '{}' not found", property_name))?;

    let prop_b = props_b.iter()
        .find(|p| p.name == property_name)
        .unwrap();

    let value_a = (prop_a.getter)(a);
    let value_b = (prop_b.getter)(b);

    // Type-specific comparison (simplified)
    if let (Some(a), Some(b)) = (value_a.downcast_ref::<f32>(), value_b.downcast_ref::<f32>()) {
        Ok((a - b).abs() < 0.001)  // Float equality with epsilon
    } else if let (Some(a), Some(b)) = (value_a.downcast_ref::<i32>(), value_b.downcast_ref::<i32>()) {
        Ok(a == b)
    } else if let (Some(a), Some(b)) = (value_a.downcast_ref::<bool>(), value_b.downcast_ref::<bool>()) {
        Ok(a == b)
    } else if let (Some(a), Some(b)) = (value_a.downcast_ref::<String>(), value_b.downcast_ref::<String>()) {
        Ok(a == b)
    } else {
        Err("Unsupported type for comparison".to_string())
    }
}
```

## Error Handling

### Graceful Property Access

```rust
pub enum PropertyError {
    NotFound(String),
    TypeMismatch { expected: String, got: String },
    AccessDenied(String),
}

impl std::fmt::Display for PropertyError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            PropertyError::NotFound(name) => write!(f, "Property '{}' not found", name),
            PropertyError::TypeMismatch { expected, got } => {
                write!(f, "Type mismatch: expected '{}', got '{}'", expected, got)
            }
            PropertyError::AccessDenied(reason) => write!(f, "Access denied: {}", reason),
        }
    }
}

impl std::error::Error for PropertyError {}

fn safe_get_property<T: 'static + Clone>(
    component: &dyn EngineClass,
    property_name: &str
) -> Result<T, PropertyError> {
    let properties = component.get_properties();

    let prop = properties.iter()
        .find(|p| p.name == property_name)
        .ok_or_else(|| PropertyError::NotFound(property_name.to_string()))?;

    let value = (prop.getter)(component);

    value.downcast_ref::<T>()
        .cloned()
        .ok_or_else(|| PropertyError::TypeMismatch {
            expected: std::any::type_name::<T>().to_string(),
            got: prop.type_info.type_name.to_string(),
        })
}

// Usage with proper error handling
match safe_get_property::<f32>(&health, "current_health") {
    Ok(value) => println!("Health: {}", value),
    Err(PropertyError::NotFound(name)) => eprintln!("Property '{}' doesn't exist", name),
    Err(PropertyError::TypeMismatch { expected, got }) => {
        eprintln!("Wrong type: wanted {}, got {}", expected, got)
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

## Integration with EngineClass

### Creating Components Dynamically

```rust
use pulsar_reflection::REGISTRY;

fn create_component_by_name(class_name: &str) -> Option<Box<dyn EngineClass>> {
    REGISTRY.create_instance(class_name)
}

// Usage
if let Some(component) = create_component_by_name("HealthComponent") {
    println!("Created: {}", component.class_name());
}
```

### Component Cloning

```rust
fn clone_component(component: &dyn EngineClass) -> Option<Box<dyn EngineClass>> {
    let class_name = component.class_name();
    let mut clone = REGISTRY.create_instance(class_name)?;

    // Copy all properties
    let properties = component.get_properties();
    for prop in properties {
        let value = (prop.getter)(component);
        (prop.setter)(clone.as_mut(), value);
    }

    Some(clone)
}
```

### Component Diff

```rust
pub struct PropertyDiff {
    pub property_name: String,
    pub old_value: String,
    pub new_value: String,
}

fn diff_components(
    old: &dyn EngineClass,
    new: &dyn EngineClass
) -> Result<Vec<PropertyDiff>, String> {
    if old.class_name() != new.class_name() {
        return Err("Cannot diff different component types".to_string());
    }

    let mut diffs = Vec::new();
    let properties = old.get_properties();

    for prop in properties {
        let old_value = (prop.getter)(old);
        let new_value = (prop.getter)(new);

        let old_str = format_any_value(&old_value);
        let new_str = format_any_value(&new_value);

        if old_str != new_str {
            diffs.push(PropertyDiff {
                property_name: prop.name.to_string(),
                old_value: old_str,
                new_value: new_str,
            });
        }
    }

    Ok(diffs)
}

fn format_any_value(value: &Box<dyn Any>) -> String {
    if let Some(f) = value.downcast_ref::<f32>() {
        format!("{}", f)
    } else if let Some(i) = value.downcast_ref::<i32>() {
        format!("{}", i)
    } else if let Some(b) = value.downcast_ref::<bool>() {
        format!("{}", b)
    } else if let Some(s) = value.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown>".to_string()
    }
}
```

## Best Practices

### Always Check Types

```rust
// ❌ Dangerous - assumes type without checking
let value = get_property(&component, "health");
let health = value.downcast_ref::<f32>().unwrap();  // Can panic!

// ✅ Safe - handles type mismatch gracefully
let value = get_property(&component, "health");
match value.downcast_ref::<f32>() {
    Some(health) => println!("Health: {}", health),
    None => eprintln!("Health property is not an f32"),
}
```

### Use Type Aliases for Clarity

```rust
type PropertyValue = Box<dyn Any>;
type PropertyName = &'static str;

fn get_property(component: &dyn EngineClass, name: PropertyName) -> Option<PropertyValue> {
    // ...
}
```

### Cache Property Lookups

```rust
struct PropertyCache {
    property_metadata: Vec<PropertyMetadata>,
}

impl PropertyCache {
    fn new(component: &dyn EngineClass) -> Self {
        Self {
            property_metadata: component.get_properties(),
        }
    }

    fn get(&self, name: &str) -> Option<&PropertyMetadata> {
        self.property_metadata.iter().find(|p| p.name == name)
    }
}

// Instead of calling get_properties() repeatedly
let cache = PropertyCache::new(&component);
for i in 0..1000 {
    if let Some(prop) = cache.get("health") {
        // Use cached metadata
    }
}
```

## Next Steps

- **[Dynamic Types](04-dynamic-types.md)**: Build types at runtime
- **[Advanced Usage](05-advanced-usage.md)**: Custom serializers, optimizations
- **[Safety Guide](06-safety-best-practices.md)**: Avoid common pitfalls

Happy coding! 🦀
