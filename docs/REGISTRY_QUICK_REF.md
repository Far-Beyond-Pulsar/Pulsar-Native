# Registry System Quick Reference

Quick copy-paste examples for common registry operations.

## Accessing the Registry

```rust
use engine_fs::global_registry;

let registry = global_registry();
```

## Creating Files

```rust
// Create with default directory
registry.create_new_file("type_alias", "MyType", None)?;

// Create in custom directory
registry.create_new_file("blueprint", "MyBP", Some(Path::new("custom/path")))?;
```

## Finding Asset Types

```rust
// By ID
if let Some(asset_type) = registry.get_asset_type("type_alias") {
    println!("{}", asset_type.display_name());
}

// By file path
if let Some(asset_type) = registry.find_asset_type_for_file(&path) {
    println!("File is: {}", asset_type.display_name());
}

// All types
for asset_type in registry.get_all_asset_types() {
    println!("{} {}", asset_type.icon(), asset_type.display_name());
}

// By category
for asset_type in registry.get_asset_types_by_category(AssetCategory::Scripts) {
    println!("Script type: {}", asset_type.display_name());
}
```

## Finding Editors

```rust
// By ID
if let Some(editor) = registry.get_editor("blueprint_editor") {
    let instance = editor.create_instance(Some(path));
}

// By file path
if let Some(editor) = registry.find_editor_for_file(&path) {
    let instance = editor.create_instance(Some(path));
}
```

## Registering New Types

```rust
// Register asset type
registry.register_asset_type(Arc::new(MyAssetType));

// Register editor
registry.register_editor(Arc::new(MyEditorType));
```

## Building Menus

```rust
// Simple list
for asset_type in registry.get_all_asset_types() {
    menu.add(
        format!("{} {}", asset_type.icon(), asset_type.display_name()),
        create_action(asset_type.type_id())
    );
}

// Grouped by category
for category in [TypeSystem, Blueprints, Scripts, Scenes, Rendering] {
    for asset_type in registry.get_asset_types_by_category(category) {
        menu.add(
            format!("{} {}", asset_type.icon(), asset_type.display_name()),
            create_action(asset_type.type_id())
        );
    }
    menu.separator();
}
```

## Implementing AssetType

```rust
pub struct MyAsset;

impl AssetType for MyAsset {
    fn type_id(&self) -> &'static str { "my_asset" }
    fn display_name(&self) -> &'static str { "My Asset" }
    fn icon(&self) -> &'static str { "🎯" }
    fn description(&self) -> &'static str { "Does something cool" }
    fn extensions(&self) -> &[&'static str] { &["myfile.json"] }
    fn default_directory(&self) -> &'static str { "assets" }
    fn category(&self) -> AssetCategory { AssetCategory::Data }
    fn editor_id(&self) -> &'static str { "my_editor" }
    
    fn generate_template(&self, name: &str) -> String {
        json!({
            "name": name,
            "version": "1.0"
        }).to_string()
    }
}
```

## Implementing EditorType

```rust
pub struct MyEditorType;

impl EditorType for MyEditorType {
    fn editor_id(&self) -> &'static str { "my_editor" }
    fn display_name(&self) -> &'static str { "My Editor" }
    fn icon(&self) -> &'static str { "📝" }
    
    fn create_instance(&self, file_path: Option<PathBuf>) -> Box<dyn EditorInstance> {
        Box::new(MyEditorInstance::new(file_path))
    }
}
```

## Common Patterns

### Check if file can be opened
```rust
if registry.find_asset_type_for_file(&path).is_some() {
    // Can open
}
```

### Get file extension for type
```rust
if let Some(asset_type) = registry.get_asset_type("blueprint") {
    let filename = asset_type.file_name("MyBlueprint");
    // "MyBlueprint.blueprint.json"
}
```

### Generate template
```rust
if let Some(asset_type) = registry.get_asset_type("type_alias") {
    let content = asset_type.generate_template("MyType");
    std::fs::write(path, content)?;
}
```

## Asset Categories

```rust
AssetCategory::TypeSystem
AssetCategory::Blueprints  
AssetCategory::Scripts
AssetCategory::Scenes
AssetCategory::Rendering
AssetCategory::Audio
AssetCategory::UI
AssetCategory::Data
AssetCategory::Config
```

## Registered Type IDs

```rust
// Type System
"type_alias"
"struct"
"enum"
"trait"

// Blueprints
"blueprint"
"blueprint_class"
"blueprint_function"

// Scripts
"rust_script"
"lua_script"

// Scenes
"scene"
"prefab"

// Rendering
"material"
"shader"
```

## Error Handling

```rust
match registry.create_new_file("type_alias", "MyType", None) {
    Ok(path) => println!("Created: {:?}", path),
    Err(e) => eprintln!("Failed: {}", e),
}
```

## Thread Safety

```rust
// Safe to call from any thread
std::thread::spawn(|| {
    let registry = global_registry();
    for asset_type in registry.get_all_asset_types() {
        println!("{}", asset_type.display_name());
    }
});
```

## Performance Tips

- Registry lookups are O(1)
- Extension mapping is O(1) + small list scan
- Enumeration is O(n) but n is small (~13 types)
- Use `get_asset_types_by_category()` to reduce iteration
- Registry is initialized once at startup

## Migration Checklist

When converting hardcoded logic:

- [ ] Replace `match ext` with `find_asset_type_for_file()`
- [ ] Replace `match kind` with registry lookup
- [ ] Replace hardcoded menus with dynamic generation
- [ ] Replace manual templates with `generate_template()`
- [ ] Replace file creation with `create_new_file()`
- [ ] Remove old enum/constants
- [ ] Test all affected code paths

## Common Mistakes

❌ **Don't do this:**
```rust
match path.extension() {
    Some("alias.json") => ...,
    Some("blueprint.json") => ...,
}
```

✅ **Do this:**
```rust
if let Some(asset_type) = registry.find_asset_type_for_file(&path) {
    // Use asset_type
}
```

❌ **Don't do this:**
```rust
let template = match kind {
    TypeAlias => "...",
    Blueprint => "...",
};
```

✅ **Do this:**
```rust
let template = asset_type.generate_template(name);
```

## Full Example: Context Menu

```rust
.context_menu(move |menu, window, cx| {
    let mut menu = menu;
    let registry = global_registry();
    
    // Group by category
    for category in [AssetCategory::TypeSystem, AssetCategory::Blueprints] {
        for asset_type in registry.get_asset_types_by_category(category) {
            let type_id = asset_type.type_id().to_string();
            let label = format!("{} {}", asset_type.icon(), asset_type.display_name());
            
            menu = menu.menu(label, Box::new(CreateAssetAction {
                type_id,
                directory: path.clone(),
            }));
        }
        menu = menu.separator();
    }
    
    menu
})
```

---

**See Also**:
- `TRAIT_REGISTRY_SYSTEM.md` - Full architecture
- `MIGRATION_STATUS.md` - What's left to migrate
- `SESSION_SUMMARY.md` - Complete overview
