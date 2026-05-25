# Custom Type Renderers for Plugins

This guide explains how to create custom property editors for plugin-defined types using the `TypeRenderer` system.

## Overview

The TypeRenderer system allows plugins to register custom UI rendering logic for their types. This enables:

- **Custom property editors**: Create specialized UI widgets (color pickers, sliders, dropdowns) for your types
- **Visual editors**: Provide rich editing experiences in the property inspector
- **Plugin extensibility**: Extend the editor without modifying engine code

## Quick Start

### 1. Derive Reflectable for Your Type

First, make sure your type implements the `Reflectable` trait:

```rust
use pulsar_reflection::Reflectable;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Reflectable, Serialize, Deserialize)]
pub struct CustomColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
```

### 2. Create a Custom Renderer

Implement the `TypeRenderer` trait:

```rust
use pulsar_reflection::{TypeRenderer, RenderResult, RuntimeTypeInfo};
use std::any::Any;

pub struct ColorPickerRenderer;

impl TypeRenderer for ColorPickerRenderer {
    fn can_render(&self, type_info: &RuntimeTypeInfo) -> bool {
        // Check if this is our CustomColor type
        type_info.type_name.ends_with("CustomColor")
    }

    fn render(
        &self,
        ui_context: &mut dyn Any,
        value: &mut dyn Any,
        type_info: &RuntimeTypeInfo,
    ) -> RenderResult {
        // Downcast to your UI framework's context (e.g., egui::Ui)
        let ui = ui_context.downcast_mut::<egui::Ui>().unwrap();

        // Downcast value to your type
        let color = value.downcast_mut::<CustomColor>().unwrap();

        // Render custom UI
        let mut color_array = [color.r, color.g, color.b, color.a];
        let changed = ui.color_edit_button_rgba_unmultiplied(&mut color_array).changed();

        if changed {
            color.r = color_array[0];
            color.g = color_array[1];
            color.b = color_array[2];
            color.a = color_array[3];
            RenderResult::Changed
        } else {
            RenderResult::Unchanged
        }
    }
}
```

### 3. Register the Renderer

Register your renderer using the `inventory` crate:

```rust
use pulsar_reflection::{TypeRendererRegistration, TYPE_RENDERER_REGISTRY};
use std::any::TypeId;
use std::sync::Arc;

// Submit registration at compile time
inventory::submit! {
    TypeRendererRegistration::new(
        TypeId::of::<CustomColor>(),
        Arc::new(ColorPickerRenderer)
    )
}
```

Or register at runtime:

```rust
use pulsar_reflection::register_type_renderer;

pub fn init_plugin() {
    register_type_renderer(
        TypeId::of::<CustomColor>(),
        Arc::new(ColorPickerRenderer)
    );
}
```

## Complete Example: Game Item Editor

Here's a complete example of a custom renderer for a game item type:

```rust
use pulsar_reflection::{Reflectable, TypeRenderer, RenderResult, RuntimeTypeInfo};
use serde::{Serialize, Deserialize};
use std::any::{Any, TypeId};
use std::sync::Arc;

// Define custom type
#[derive(Debug, Clone, Reflectable, Serialize, Deserialize)]
pub struct GameItem {
    pub name: String,
    pub icon_path: String,
    pub value: i32,
    pub weight: f32,
    pub rarity: ItemRarity,
}

#[derive(Debug, Clone, Reflectable, Serialize, Deserialize, PartialEq)]
pub enum ItemRarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

// Create custom renderer
pub struct GameItemRenderer;

impl TypeRenderer for GameItemRenderer {
    fn can_render(&self, type_info: &RuntimeTypeInfo) -> bool {
        type_info.type_name.ends_with("GameItem")
    }

    fn render(
        &self,
        ui_context: &mut dyn Any,
        value: &mut dyn Any,
        _type_info: &RuntimeTypeInfo,
    ) -> RenderResult {
        let ui = ui_context.downcast_mut::<egui::Ui>().unwrap();
        let item = value.downcast_mut::<GameItem>().unwrap();

        let mut changed = false;

        // Name field
        ui.label("Item Name:");
        changed |= ui.text_edit_singleline(&mut item.name).changed();

        // Icon path with button
        ui.horizontal(|ui| {
            ui.label("Icon:");
            changed |= ui.text_edit_singleline(&mut item.icon_path).changed();
            if ui.button("Browse...").clicked() {
                // TODO: Open file picker
            }
        });

        // Value with drag widget
        ui.label("Value:");
        changed |= ui.add(egui::DragValue::new(&mut item.value).clamp_range(0..=999999)).changed();

        // Weight slider
        ui.label("Weight (kg):");
        changed |= ui.add(egui::Slider::new(&mut item.weight, 0.0..=100.0).suffix(" kg")).changed();

        // Rarity dropdown with color coding
        ui.label("Rarity:");
        let rarity_color = match item.rarity {
            ItemRarity::Common => egui::Color32::GRAY,
            ItemRarity::Uncommon => egui::Color32::GREEN,
            ItemRarity::Rare => egui::Color32::BLUE,
            ItemRarity::Epic => egui::Color32::from_rgb(148, 0, 211), // Purple
            ItemRarity::Legendary => egui::Color32::from_rgb(255, 165, 0), // Orange
        };

        egui::ComboBox::from_id_salt("rarity")
            .selected_text(egui::RichText::new(format!("{:?}", item.rarity)).color(rarity_color))
            .show_ui(ui, |ui| {
                for rarity in [
                    ItemRarity::Common,
                    ItemRarity::Uncommon,
                    ItemRarity::Rare,
                    ItemRarity::Epic,
                    ItemRarity::Legendary,
                ] {
                    let color = match rarity {
                        ItemRarity::Common => egui::Color32::GRAY,
                        ItemRarity::Uncommon => egui::Color32::GREEN,
                        ItemRarity::Rare => egui::Color32::BLUE,
                        ItemRarity::Epic => egui::Color32::from_rgb(148, 0, 211),
                        ItemRarity::Legendary => egui::Color32::from_rgb(255, 165, 0),
                    };

                    if ui.selectable_label(
                        item.rarity == rarity,
                        egui::RichText::new(format!("{:?}", rarity)).color(color)
                    ).clicked() {
                        item.rarity = rarity;
                        changed = true;
                    }
                }
            });

        if changed {
            RenderResult::Changed
        } else {
            RenderResult::Unchanged
        }
    }
}

// Register the renderer
inventory::submit! {
    pulsar_reflection::TypeRendererRegistration::new(
        TypeId::of::<GameItem>(),
        Arc::new(GameItemRenderer)
    )
}
```

## Using Custom Types in Blueprints

Once you have a Reflectable type with a custom renderer, you can use it in blueprint nodes:

```rust
use pulsar_std::{blueprint, NodeTypes};

#[blueprint(type: NodeTypes::pure, category: "Items")]
pub fn create_sword() -> GameItem {
    GameItem {
        name: "Iron Sword".to_string(),
        icon_path: "assets/icons/iron_sword.png".to_string(),
        value: 100,
        weight: 2.5,
        rarity: ItemRarity::Common,
    }
}

#[blueprint(type: NodeTypes::pure, category: "Items")]
pub fn upgrade_item(item: GameItem) -> GameItem {
    let mut upgraded = item;
    upgraded.rarity = match upgraded.rarity {
        ItemRarity::Common => ItemRarity::Uncommon,
        ItemRarity::Uncommon => ItemRarity::Rare,
        ItemRarity::Rare => ItemRarity::Epic,
        ItemRarity::Epic => ItemRarity::Legendary,
        ItemRarity::Legendary => ItemRarity::Legendary,
    };
    upgraded.value *= 2;
    upgraded
}
```

## Best Practices

### 1. Type Checking

Always validate types before downcasting:

```rust
fn can_render(&self, type_info: &RuntimeTypeInfo) -> bool {
    type_info.type_id == TypeId::of::<MyType>() ||
    type_info.type_name.ends_with("MyType")
}
```

### 2. Error Handling

Handle downcast failures gracefully:

```rust
fn render(&self, ui_context: &mut dyn Any, value: &mut dyn Any, _type_info: &RuntimeTypeInfo) -> RenderResult {
    let ui = match ui_context.downcast_mut::<egui::Ui>() {
        Some(ui) => ui,
        None => return RenderResult::Unchanged,
    };

    let my_value = match value.downcast_mut::<MyType>() {
        Some(v) => v,
        None => return RenderResult::Unchanged,
    };

    // Render UI...
}
```

### 3. Return Correct Results

Always return `RenderResult::Changed` when the value is modified:

```rust
let mut changed = false;
changed |= ui.text_edit_singleline(&mut value.name).changed();
changed |= ui.drag_value(&mut value.count).changed();

if changed {
    RenderResult::Changed
} else {
    RenderResult::Unchanged
}
```

### 4. UI Framework Agnostic

The system is designed to be UI framework agnostic. Use `dyn Any` for the UI context:

- For egui: `downcast_mut::<egui::Ui>()`
- For gpui: `downcast_mut::<gpui::WindowContext>()`
- For custom frameworks: `downcast_mut::<YourContext>()`

## Integration with Blueprint System

### Automatic Type Registration

Types with `#[derive(Reflectable)]` are automatically registered in the `RUNTIME_TYPE_REGISTRY`:

```rust
#[derive(Reflectable, Serialize, Deserialize)]
pub struct MyType {
    pub value: f32,
}

// Automatically registered at startup!
```

### Blueprint Node Metadata

The `#[blueprint]` macro automatically captures type information:

```rust
#[blueprint(type: NodeTypes::pure, category: "Custom")]
pub fn process_my_type(input: MyType) -> MyType {
    input
}

// The macro generates:
// - Runtime type info accessors for parameters
// - Runtime type info accessors for return type
// - Size and alignment metadata
```

### Type Validation

Validate that all blueprint nodes have registered types:

```rust
use pulsar_std::registry::validate_all_node_types;

pub fn init_plugin() {
    let issues = validate_all_node_types();
    if !issues.is_empty() {
        for (node_name, unregistered_types) in issues {
            eprintln!("Node '{}' has unregistered types: {:?}", node_name, unregistered_types);
        }
    }
}
```

## Debugging

### Check Type Registration

```rust
use pulsar_reflection::RUNTIME_TYPE_REGISTRY;
use std::any::TypeId;

let type_info = RUNTIME_TYPE_REGISTRY.get::<MyType>();
assert!(type_info.is_some(), "MyType should be registered");
```

### Check Renderer Registration

```rust
use pulsar_reflection::TYPE_RENDERER_REGISTRY;

let registry = TYPE_RENDERER_REGISTRY.lock().unwrap();
assert!(registry.has_renderer(TypeId::of::<MyType>()));
```

### List All Registered Types

```rust
use pulsar_std::registry::get_all_nodes;

for node in get_all_nodes() {
    println!("Node: {}", node.name);
    for param in node.params {
        if let Some(type_info) = param.get_type_info() {
            println!("  Param '{}': {}", param.name, type_info.type_name);
        }
    }
}
```

## FAQ

**Q: Can I have multiple renderers for the same type?**

A: No, each type can only have one registered renderer. The last registration wins.

**Q: What if I don't provide a custom renderer?**

A: The engine will use the default type-driven renderer based on the RuntimeTypeInfo structure.

**Q: Can custom renderers work with generic types?**

A: No, custom renderers are registered per concrete type. Generic types need to be instantiated first.

**Q: How do I handle nested custom types?**

A: The default renderer will recursively render nested types. You can also create renderers that handle nested types explicitly.

**Q: Can plugins override engine type renderers?**

A: Yes, but it's not recommended. Plugin renderers can register for any TypeId, including engine types.

## See Also

- [Runtime Type Reflection](00-overview.md)
- [Reflectable Trait](01-reflectable-trait.md)
- [Blueprint System Documentation](../pulsar_std/README.md)
- [Plugin Development Guide](../../docs/plugin-development.md)
