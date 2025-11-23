# Trait-Based Editor and Asset Registry System

## Overview

The engine now has a clean, trait-based system for registering asset types and their editors. This architecture enables:
- **Plugin-ready design**: Future DLL-based plugins can easily register new asset types
- **Zero central coupling**: The engine core never references concrete editor types
- **Type-safe file handling**: Automatic routing of files to appropriate editors
- **Centralized asset management**: Single source of truth for all asset operations

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                   engine_fs (Core)                      │
│  ┌──────────────────────────────────────────────────┐  │
│  │          AssetRegistry (Global Singleton)        │  │
│  │                                                  │  │
│  │  - AssetType trait implementations              │  │
│  │  - EditorType trait implementations             │  │
│  │  - Extension → Type mapping                     │  │
│  │  - Type → Editor mapping                        │  │
│  └──────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
                          ▲
                          │
          ┌───────────────┴───────────────┐
          │                               │
┌─────────┴────────────┐    ┌────────────┴─────────────┐
│  ui_alias_editor     │    │  ui_editor (blueprints)  │
│                      │    │                          │
│  TypeAliasAssetType  │    │  BlueprintAssetType      │
│  TypeAliasEditorType │    │  BlueprintEditorType     │
│                      │    │                          │
│  Registers metadata  │    │  Registers metadata      │
│  with registry       │    │  with registry           │
└──────────────────────┘    └──────────────────────────┘
          │                               │
          └───────────────┬───────────────┘
                          ▼
┌─────────────────────────────────────────────────────────┐
│               ui_core (Application Layer)               │
│  ┌──────────────────────────────────────────────────┐  │
│  │           PulsarApp::editor_openers              │  │
│  │                                                  │  │
│  │  Maps editor_id → actual opening function       │  │
│  │  Registered in new_internal()                   │  │
│  └──────────────────────────────────────────────────┘  │
│                                                         │
│  registry_init::register_all_asset_types()             │
│  - Called once at app startup                          │
│  - Registers all AssetType and EditorType traits       │
└─────────────────────────────────────────────────────────┘
```

## Key Traits

### AssetType Trait
```rust
pub trait AssetType: Send + Sync {
    fn type_id(&self) -> &'static str;         // e.g., "type_alias"
    fn display_name(&self) -> &'static str;     // e.g., "Type Alias"
    fn icon(&self) -> &'static str;             // e.g., "📝"
    fn description(&self) -> &'static str;      
    fn extensions(&self) -> &[&'static str];    // e.g., ["alias.json"]
    fn default_directory(&self) -> &'static str; // e.g., "types/aliases"
    fn category(&self) -> AssetCategory;        
    fn generate_template(&self, name: &str) -> String;
    fn editor_id(&self) -> &'static str;        // Links to editor
}
```

### EditorType Trait
```rust
pub trait EditorType: Send + Sync {
    fn editor_id(&self) -> &'static str;      // e.g., "type_alias_editor"
    fn display_name(&self) -> &'static str;   
    fn icon(&self) -> &'static str;           
    fn clone_box(&self) -> Box<dyn EditorType>;
}
```

## How to Add a New Editor/Asset Type

### Step 1: Implement the Traits (in editor crate)

Example from `ui_alias_editor/src/registry.rs`:

```rust
use engine_fs::registry::{AssetType, EditorType, AssetCategory};

#[derive(Clone)]
pub struct TypeAliasEditorType;

impl EditorType for TypeAliasEditorType {
    fn editor_id(&self) -> &'static str { "type_alias_editor" }
    fn display_name(&self) -> &'static str { "Type Alias Editor" }
    fn icon(&self) -> &'static str { "📝" }
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

#[derive(Clone)]
pub struct TypeAliasAssetType;

impl AssetType for TypeAliasAssetType {
    fn type_id(&self) -> &'static str { "type_alias" }
    fn display_name(&self) -> &'static str { "Type Alias" }
    fn icon(&self) -> &'static str { "📝" }
    fn description(&self) -> &'static str { "Rust type alias definition" }
    fn extensions(&self) -> &[&'static str] { &["alias.json"] }
    fn default_directory(&self) -> &'static str { "types/aliases" }
    fn category(&self) -> AssetCategory { AssetCategory::TypeSystem }
    fn editor_id(&self) -> &'static str { "type_alias_editor" }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "type": "String",
            "description": ""
        }).to_string()
    }
}
```

### Step 2: Register with Global Registry (in ui_core/src/registry_init.rs)

```rust
use your_editor::{YourAssetType, YourEditorType};

pub fn register_all_asset_types() {
    let registry = global_registry();
    
    // Add your types
    registry.register_asset_type(Arc::new(YourAssetType));
    registry.register_editor(Arc::new(YourEditorType));
}
```

### Step 3: Register Editor Opener (in ui_core/src/app.rs - new_internal())

```rust
// In the editor_openers HashMap initialization:
editor_openers.insert(
    "your_editor_id".to_string(),
    Arc::new(Box::new(|app: &mut PulsarApp, path, window, cx| {
        app.open_your_tab(path, window, cx);
    }))
);
```

## File Opening Flow

When a user opens a file:

1. **Registry Lookup**: `registry.find_asset_type_for_file(path)` matches file extension
2. **Editor ID Resolution**: Asset type returns its `editor_id()`
3. **Opener Invocation**: App looks up `editor_openers[editor_id]` and calls it
4. **Tab Creation**: Specific editor method (e.g., `open_alias_tab`) creates the UI

## Benefits

### For Core Engine
- **Zero concrete dependencies**: Engine never imports specific editor crates
- **Extensible**: New editors = just implement traits
- **Type-safe**: Compiler ensures all required methods are implemented

### For Editor Developers  
- **Self-contained**: Each editor crate defines its own types
- **Clear contract**: Trait signatures document exactly what's needed
- **No coordination**: No need to modify central switch statements

### For Users
- **Consistent**: All file operations work the same way
- **Discoverable**: Asset categories and descriptions aid navigation
- **Predictable**: File extensions automatically route to correct editors

## Migration Status

### ✅ Completed
- engine_fs crate with trait definitions
- Type alias editor registration
- Global registry singleton
- Basic file opening flow

### 🚧 TODO
- Register remaining asset types:
  - Blueprint (blueprint_class, blueprint_function)
  - Script (rust_script, lua_script, shader)
  - Struct
  - Enum
  - Trait
  - DAW
  - Table/Database
  - Level editor
  - Specialized editors (material, particle, terrain, etc.)
- Remove hardcoded switch statements from app.rs
- Add file creation UI that uses registry categories
- Implement search/filter by asset category
- Add registry-based "new file" menu in file drawer

## Future: Plugin Support

With this architecture, plugins can register new asset types at runtime:

```rust
// In plugin DLL
#[no_mangle]
pub extern "C" fn plugin_init(registry: &AssetRegistry) {
    registry.register_asset_type(Arc::new(MyCustomAssetType));
    registry.register_editor(Arc::new(MyCustomEditorType));
}
```

The engine core never needs to know about the plugin's types!
