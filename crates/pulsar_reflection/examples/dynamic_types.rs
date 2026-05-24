//! Example demonstrating the dynamic type composition system
//!
//! This example shows how to:
//! - Build runtime-composed types from compile-time types
//! - Create instances with type-safe field access
//! - Register types globally
//! - Use dynamic types for modding/data-driven design

use pulsar_reflection::{
    DynamicTypeBuilder, DynamicValue, Reflectable, DYNAMIC_TYPE_REGISTRY, RUNTIME_TYPE_REGISTRY,
};

fn main() {
    println!("=== Dynamic Type Composition Example ===\n");

    // Example 1: Creating a custom material type at runtime
    example_custom_material();

    println!("\n{}\n", "=".repeat(60));

    // Example 2: Data-driven entity definitions
    example_entity_definition();

    println!("\n{}\n", "=".repeat(60));

    // Example 3: Runtime schema evolution
    example_schema_evolution();
}

/// Example 1: Creating a custom material type at runtime
/// This demonstrates how a modding system could define new material types
fn example_custom_material() {
    println!("Example 1: Custom Material Type\n");

    // Get compile-time type info for the field types we'll use
    let f32_info = <f32>::type_info();
    let color_info = <[f32; 4]>::type_info();
    let string_info = <String>::type_info();

    // Build a new material type at runtime
    let material_type = DynamicTypeBuilder::new("CustomWoodMaterial")
        .add_field("albedo", color_info)
        .add_field("roughness", f32_info)
        .add_field("metallic", f32_info)
        .add_field("normal_strength", f32_info)
        .add_field("texture_path", string_info)
        .build();

    println!("Created dynamic type: {}", material_type.name);
    println!("  Fields: {}", material_type.fields.len());
    println!("  Size: {} bytes", material_type.total_size);
    println!("  Alignment: {} bytes", material_type.total_align);
    println!();

    // Register the type globally
    let type_uuid = DYNAMIC_TYPE_REGISTRY.register(material_type.clone());
    println!("Registered with UUID: {}", type_uuid);
    println!();

    // Create an instance
    let mut wood_material = DynamicValue::new(material_type);

    // Set field values (type-safe!)
    wood_material
        .set_field("albedo", Box::new([0.6, 0.4, 0.2, 1.0]))
        .expect("Failed to set albedo");

    wood_material
        .set_field("roughness", Box::new(0.8f32))
        .expect("Failed to set roughness");

    wood_material
        .set_field("metallic", Box::new(0.0f32))
        .expect("Failed to set metallic");

    wood_material
        .set_field("normal_strength", Box::new(1.0f32))
        .expect("Failed to set normal_strength");

    wood_material
        .set_field("texture_path", Box::new("textures/wood_oak.png".to_string()))
        .expect("Failed to set texture_path");

    println!("Created material instance with values:");

    // Read back the values
    let roughness = wood_material
        .get_field_typed::<f32>("roughness")
        .expect("Failed to get roughness");
    println!("  Roughness: {}", roughness);

    let albedo = wood_material
        .get_field_typed::<[f32; 4]>("albedo")
        .expect("Failed to get albedo");
    println!("  Albedo: {:?}", albedo);

    let texture = wood_material
        .get_field_typed::<String>("texture_path")
        .expect("Failed to get texture_path");
    println!("  Texture: {}", texture);

    // Demonstrate type safety - this will fail
    println!();
    let result = wood_material.set_field("roughness", Box::new(42i32));
    match result {
        Ok(_) => println!("❌ Type check failed!"),
        Err(e) => println!("✅ Type safety enforced: {}", e),
    }
}

/// Example 2: Data-driven entity definitions
/// This shows how game designers could define entity types without code
fn example_entity_definition() {
    println!("Example 2: Data-Driven Entity Definitions\n");

    // Simulate loading an entity definition from JSON
    let entity_json = r#"{
        "name": "Goblin",
        "fields": [
            {"name": "health", "type": "f32"},
            {"name": "max_health", "type": "f32"},
            {"name": "attack_damage", "type": "f32"},
            {"name": "movement_speed", "type": "f32"},
            {"name": "is_hostile", "type": "bool"}
        ]
    }"#;

    println!("Loading entity definition from JSON:");
    println!("{}\n", entity_json);

    // Parse the definition
    let definition: serde_json::Value =
        serde_json::from_str(entity_json).expect("Failed to parse JSON");

    // Build the dynamic type
    let mut builder = DynamicTypeBuilder::new(definition["name"].as_str().unwrap());

    for field in definition["fields"].as_array().unwrap() {
        let field_name = field["name"].as_str().unwrap();
        let field_type = field["type"].as_str().unwrap();

        // Look up the compile-time type by name
        let type_info = RUNTIME_TYPE_REGISTRY
            .get_by_name(field_type)
            .expect(&format!("Unknown type: {}", field_type));

        builder = builder.add_field(field_name, type_info);
    }

    let goblin_type = builder.build();

    println!("Created entity type: {}", goblin_type.name);
    println!("  Fields:");
    for field in &goblin_type.fields {
        println!(
            "    - {} : {} (offset: {}, size: {})",
            field.name, field.base_type.type_name, field.offset, field.base_type.size
        );
    }
    println!();

    // Create a goblin instance
    let mut goblin = DynamicValue::new(goblin_type);
    goblin.set_field("health", Box::new(50.0f32)).unwrap();
    goblin
        .set_field("max_health", Box::new(50.0f32))
        .unwrap();
    goblin
        .set_field("attack_damage", Box::new(12.0f32))
        .unwrap();
    goblin
        .set_field("movement_speed", Box::new(3.5f32))
        .unwrap();
    goblin.set_field("is_hostile", Box::new(true)).unwrap();

    println!("Spawned a Goblin:");
    println!(
        "  Health: {}",
        goblin.get_field_typed::<f32>("health").unwrap()
    );
    println!(
        "  Attack: {}",
        goblin.get_field_typed::<f32>("attack_damage").unwrap()
    );
    println!(
        "  Speed: {}",
        goblin.get_field_typed::<f32>("movement_speed").unwrap()
    );
    println!(
        "  Hostile: {}",
        goblin.get_field_typed::<bool>("is_hostile").unwrap()
    );
}

/// Example 3: Runtime schema evolution
/// This demonstrates versioning and migrating data structures
fn example_schema_evolution() {
    println!("Example 3: Runtime Schema Evolution\n");

    // Version 1: Simple player stats
    let stats_v1 = DynamicTypeBuilder::new("PlayerStats_v1")
        .add_field("health", <f32>::type_info())
        .add_field("mana", <f32>::type_info())
        .build();

    println!("Created PlayerStats v1:");
    println!("  Fields: {:?}", stats_v1.fields.iter().map(|f| &f.name).collect::<Vec<_>>());

    // Create a v1 instance (simulating old save data)
    let mut old_stats = DynamicValue::new(stats_v1);
    old_stats.set_field("health", Box::new(100.0f32)).unwrap();
    old_stats.set_field("mana", Box::new(50.0f32)).unwrap();

    println!("\nOld save data:");
    println!("  Health: {}", old_stats.get_field_typed::<f32>("health").unwrap());
    println!("  Mana: {}", old_stats.get_field_typed::<f32>("mana").unwrap());

    // Version 2: Extended stats with new fields
    let stats_v2 = DynamicTypeBuilder::new("PlayerStats_v2")
        .add_field("health", <f32>::type_info())
        .add_field("mana", <f32>::type_info())
        .add_field("stamina", <f32>::type_info()) // New!
        .add_field("shield", <f32>::type_info()) // New!
        .build();

    println!("\n\nCreated PlayerStats v2 (with new fields):");
    println!("  Fields: {:?}", stats_v2.fields.iter().map(|f| &f.name).collect::<Vec<_>>());

    // Migrate v1 data to v2
    let mut new_stats = DynamicValue::new(stats_v2);

    // Copy common fields
    if let Ok(health) = old_stats.get_field_typed::<f32>("health") {
        new_stats
            .set_field("health", Box::new(*health))
            .unwrap();
    }
    if let Ok(mana) = old_stats.get_field_typed::<f32>("mana") {
        new_stats.set_field("mana", Box::new(*mana)).unwrap();
    }

    // Initialize new fields with defaults
    new_stats.set_field("stamina", Box::new(100.0f32)).unwrap();
    new_stats.set_field("shield", Box::new(0.0f32)).unwrap();

    println!("\nMigrated save data to v2:");
    println!("  Health: {} (migrated)", new_stats.get_field_typed::<f32>("health").unwrap());
    println!("  Mana: {} (migrated)", new_stats.get_field_typed::<f32>("mana").unwrap());
    println!("  Stamina: {} (default)", new_stats.get_field_typed::<f32>("stamina").unwrap());
    println!("  Shield: {} (default)", new_stats.get_field_typed::<f32>("shield").unwrap());

    println!("\n✅ Successfully evolved schema from v1 to v2!");
}
