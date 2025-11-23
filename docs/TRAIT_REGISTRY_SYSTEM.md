# Trait-Based Registry System

**Clean, extensible architecture for file types and editors**

## Overview

The Pulsar Engine now uses a trait-based registry system that eliminates messy switch statements and hardcoded file type handling. Everything is registered dynamically through traits.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│              Global Asset Registry                       │
│  (Singleton, thread-safe, initialized at startup)        │
└────────────┬────────────────────────────────────────────┘
             │
    ┌────────┴────────┐
    │                 │
┌───┴────────┐   ┌────┴─────────┐
│ AssetTypes │   │ EditorTypes  │
│            │   │              │
│ - TypeAlias│   │ - TypeAlias  │
│ - Blueprint│   │   Editor     │
│ - RustScript│  │ - Blueprint  │
│ - Scene    │   │   Editor     │
│ - Material │   │ - Script     │
│ - etc...   │   │   Editor     │
└────────────┘   │ - Level      │
                 │   Editor     │
                 │ - etc...     │
                 └──────────────┘
```

## Core Traits

### `AssetType` Trait

Defines everything about a file type:

```rust
pub trait AssetType: Send + Sync {
    fn type_id(&self) -> &'static str;           // Unique ID: "type_alias"
    fn display_name(&self) -> &'static str;      // UI name: "Type Alias"
    fn icon(&self) -> &'static str;              // Icon: "🔗"
    fn description(&self) -> &'static str;       // Tooltip text
    fn extensions(&self) -> &[&'static str];     // ["alias.json"]
    fn default_directory(&self) -> &'static str; // "types/aliases"
    fn category(&self) -> AssetCategory;         // TypeSystem
    fn editor_id(&self) -> &'static str;         // "type_alias_editor"
    fn generate_template(&self, name: &str) -> String; // Creates blank file
    
    // Provided methods
    fn can_open(&self, path: &Path) -> bool;
    fn file_name(&self, name: &str) -> String;
}
```

### `EditorType` Trait

Defines an editor that can handle one or more asset types:

```rust
pub trait EditorType: Send + Sync {
    fn editor_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn icon(&self) -> &'static str;
    fn create_instance(&self, file_path: Option<PathBuf>) -> Box<dyn EditorInstance>;
    fn supports_multi_file(&self) -> bool { false }
}
```

### `EditorInstance` Trait

An actual editor instance (the component in a tab):

```rust
pub trait EditorInstance: Send + Sync {
    fn editor_id(&self) -> &'static str;
    fn file_path(&self) -> Option<&PathBuf>;
    fn title(&self) -> String;
    fn is_modified(&self) -> bool;
    fn save(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}
```

## Implementation Example

### Creating a New Asset Type

```rust
pub struct TypeAliasAsset;

impl AssetType for TypeAliasAsset {
    fn type_id(&self) -> &'static str { "type_alias" }
    fn display_name(&self) -> &'static str { "Type Alias" }
    fn icon(&self) -> &'static str { "🔗" }
    fn description(&self) -> &'static str { "Create a reusable type definition" }
    fn extensions(&self) -> &[&'static str] { &["alias.json"] }
    fn default_directory(&self) -> &'static str { "types/aliases" }
    fn category(&self) -> AssetCategory { AssetCategory::TypeSystem }
    fn editor_id(&self) -> &'static str { "type_alias_editor" }
    
    fn generate_template(&self, name: &str) -> String {
        json!({
            "name": name,
            "display_name": name,
            "description": "",
            "ast": { "nodeKind": "Primitive", "name": "i32" }
        }).to_string()
    }
}
```

### Registering at Startup

```rust
// In main.rs or app initialization
use engine_fs::{global_registry, register_all_assets};

fn main() {
    // Register all built-in asset types
    register_all_assets(global_registry());
    
    // Or register individually
    global_registry().register_asset_type(Arc::new(TypeAliasAsset));
    
    // Register editors
    global_registry().register_editor(Arc::new(TypeAliasEditorType));
}
```

## Usage

### Creating Files

**Before (hardcoded mess):**
```rust
match asset_kind {
    AssetKind::TypeAlias => {
        let template = json!({ ... });
        std::fs::write(path, template)?;
    }
    AssetKind::Blueprint => {
        let template = json!({ ... });
        std::fs::write(path, template)?;
    }
    // ... 20+ more cases
}
```

**After (clean trait call):**
```rust
let registry = global_registry();
registry.create_new_file("type_alias", "MyType", Some(&dir))?;
```

### Opening Files

**Before:**
```rust
let ext = path.extension()?;
match ext {
    "alias.json" => open_type_alias_editor(path),
    "blueprint.json" => open_blueprint_editor(path),
    "rs" => open_script_editor(path),
    // ... 20+ more cases
}
```

**After:**
```rust
let registry = global_registry();
if let Some(editor) = registry.find_editor_for_file(&path) {
    let instance = editor.create_instance(Some(path));
    // Add to tab bar
}
```

### Building Context Menus

**Before:**
```rust
// Hardcoded list
menu.add("New Type Alias", ...);
menu.add("New Blueprint", ...);
menu.add("New Script", ...);
// ... etc
```

**After:**
```rust
let registry = global_registry();
for asset_type in registry.get_asset_types_by_category(category) {
    let label = format!("{} {}", asset_type.icon(), asset_type.display_name());
    menu.add(label, create_action(asset_type.type_id()));
}
```

## Built-in Asset Types

Currently registered:

### Type System (📝)
- `type_alias` - Type Alias (🔗)
- `struct` - Struct (📦)
- `enum` - Enum (🎯)
- `trait` - Trait (🔧)

### Blueprints (🔷)
- `blueprint` - Blueprint (🔷)
- `blueprint_class` - Blueprint Class (📘)
- `blueprint_function` - Blueprint Function (⚡)

### Scripts (📜)
- `rust_script` - Rust Script (🦀)
- `lua_script` - Lua Script (🌙)

### Scenes (🎬)
- `scene` - Scene (🎬)
- `prefab` - Prefab (🎁)

### Rendering (🎨)
- `material` - Material (🎨)
- `shader` - Shader (✨)

## Benefits

### 1. Zero Central Switch Statements

No more giant match statements. Everything is driven by traits and the registry.

### 2. Easy to Extend

Add new file types without touching core code:

```rust
// In your plugin/module
struct MyCustomAsset;
impl AssetType for MyCustomAsset { ... }

// Register it
global_registry().register_asset_type(Arc::new(MyCustomAsset));
```

### 3. Type-Safe

The type system ensures all required methods are implemented.

### 4. Dynamic at Runtime

File types can be added/removed at runtime (for plugins).

### 5. Clean Separation

- `engine_fs` - Core registry (no UI dependencies)
- `ui_editor` - UI components use the registry
- `ui_core` - App initialization registers types

## Migration Checklist

- [x] Create trait system (`registry.rs`)
- [x] Implement all asset types (`asset_impls.rs`)
- [x] Register assets at startup
- [x] Migrate file explorer context menu
- [ ] Migrate file drawer in blueprint editor
- [ ] Migrate tab opening logic
- [ ] Migrate "New File" buttons
- [ ] Remove old `AssetKind` enum
- [ ] Remove old `asset_templates.rs`
- [ ] Update all hardcoded file type checks

## API Reference

### AssetRegistry Methods

```rust
// Registration
pub fn register_asset_type(&self, asset_type: Arc<dyn AssetType>)
pub fn register_editor(&self, editor: Arc<dyn EditorType>)

// Lookup
pub fn get_asset_type(&self, type_id: &str) -> Option<Arc<dyn AssetType>>
pub fn get_editor(&self, editor_id: &str) -> Option<Arc<dyn EditorType>>
pub fn find_asset_type_for_file(&self, path: &Path) -> Option<Arc<dyn AssetType>>
pub fn find_editor_for_file(&self, path: &Path) -> Option<Arc<dyn EditorType>>

// Enumeration
pub fn get_all_asset_types(&self) -> Vec<Arc<dyn AssetType>>
pub fn get_asset_types_by_category(&self, category: AssetCategory) -> Vec<Arc<dyn AssetType>>
pub fn get_all_editors(&self) -> Vec<Arc<dyn EditorType>>

// Operations
pub fn create_new_file(&self, type_id: &str, name: &str, directory: Option<&Path>) -> Result<PathBuf>
```

## Example: Adding a New Asset Type

Let's add support for JSON data files:

```rust
// 1. Implement AssetType
pub struct JsonDataAsset;

impl AssetType for JsonDataAsset {
    fn type_id(&self) -> &'static str { "json_data" }
    fn display_name(&self) -> &'static str { "JSON Data" }
    fn icon(&self) -> &'static str { "📄" }
    fn description(&self) -> &'static str { "Generic JSON data file" }
    fn extensions(&self) -> &[&'static str] { &["json"] }
    fn default_directory(&self) -> &'static str { "data" }
    fn category(&self) -> AssetCategory { AssetCategory::Data }
    fn editor_id(&self) -> &'static str { "json_editor" }
    
    fn generate_template(&self, name: &str) -> String {
        json!({
            "name": name,
            "version": "1.0",
            "data": {}
        }).to_string()
    }
}

// 2. Register it
global_registry().register_asset_type(Arc::new(JsonDataAsset));

// 3. It immediately appears in:
//    - Context menus
//    - File creation dialogs
//    - File type detection
//    - Editor opening logic
```

## Future Enhancements

- **Plugin System**: Load asset types from plugins
- **Custom Categories**: Allow custom categories beyond built-in ones
- **Asset Dependencies**: Track which assets depend on others
- **Asset Validation**: Validate files match their type
- **Asset Migration**: Upgrade old file formats
- **Asset Thumbnails**: Generate previews for each type
- **Asset Search**: Full-text search across all assets

## Conclusion

The trait-based registry system provides a clean, extensible architecture that eliminates central hardcoded logic. All file type handling is now declarative and modular.

**Key Principle**: *Don't write code that knows about file types. Write file types that implement traits.*
