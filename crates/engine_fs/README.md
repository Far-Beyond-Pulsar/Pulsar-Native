# Engine Filesystem Layer (`engine_fs`)

Centralized asset management and indexing system for Pulsar Engine.

## Overview

The `engine_fs` crate provides a unified interface for all file system operations related to engine assets. It maintains up-to-date indexes for quick lookups and ensures consistency across the application.

## Key Features

### 1. Type Alias Index
- **Global Uniqueness**: Type alias names must be globally unique
- **Fast Lookups**: O(1) access by name
- **Search Support**: Full-text search across names, descriptions, and type expressions
- **Auto-sync**: Automatically stays synchronized with file system changes

### 2. Asset Registry
- Tracks all asset types (structs, enums, traits, aliases)
- Provides unified query interface
- Maintains file path associations

### 3. File Operations
- All file operations go through `AssetOperations`
- Automatic index updates on create/update/delete
- Name uniqueness validation
- Transactional consistency

### 4. File System Watching
- Monitors project directory for changes
- Automatically updates indexes when files are modified externally
- Handles file creation, modification, and deletion

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                          EngineFs                            │
│  Central coordinator for all asset operations                │
└────────────┬────────────────────────────────────┬───────────┘
             │                                     │
    ┌────────┴────────┐                   ┌───────┴────────┐
    │  TypeAliasIndex │                   │ AssetRegistry  │
    │                 │                   │                │
    │ - Fast lookups  │                   │ - Structs      │
    │ - Search        │                   │ - Enums        │
    │ - Validation    │                   │ - Traits       │
    └─────────────────┘                   └────────────────┘
             │                                     │
    ┌────────┴─────────────────────────────────────┴───────┐
    │              AssetOperations                          │
    │  Handles all file operations + index maintenance      │
    └───────────────────────────────────────────────────────┘
             │
    ┌────────┴────────┐
    │  File Watchers  │
    │  Auto-sync      │
    └─────────────────┘
```

## Usage

### Initialization

```rust
use engine_fs::EngineFs;
use std::path::PathBuf;

// Create EngineFs for a project
let engine_fs = EngineFs::new(PathBuf::from("/path/to/project"))?;

// Start file watching for auto-updates
engine_fs.start_watching()?;
```

### Type Alias Operations

```rust
// Create a new type alias
let content = serde_json::to_string(&alias_asset)?;
let file_path = engine_fs.operations().create_type_alias("MyType", &content)?;

// Update existing alias
engine_fs.operations().update_type_alias(&file_path, &new_content)?;

// Delete alias
engine_fs.operations().delete_type_alias(&file_path)?;
```

### Querying Type Aliases

```rust
// Get by name
if let Some(alias) = engine_fs.type_index().get("MyType") {
    println!("Found: {} at {:?}", alias.display_name, alias.file_path);
}

// Search
let results = engine_fs.type_index().search("Vec");
for alias in results {
    println!("{}: {}", alias.name, alias.type_expr);
}

// Get all
let all_aliases = engine_fs.type_index().get_all();

// Check name availability
if engine_fs.type_index().is_name_available("NewType") {
    // Name is available
}
```

### Validation

```rust
// Validate name before saving
engine_fs.type_index().validate_name("MyType", &file_path)?;
```

## Type Alias Signature

The `TypeAliasSignature` struct provides quick access to type information:

```rust
pub struct TypeAliasSignature {
    pub name: String,              // Unique ID
    pub display_name: String,      // UI display
    pub description: String,       // Human description
    pub file_path: PathBuf,        // Source file
    pub type_expr: String,         // Readable expression
    pub ast: Option<TypeAstNode>,  // Full AST
    pub last_modified: SystemTime, // Timestamp
}
```

## Integration Points

### UI Layer
- UI components use `EngineFs` for all asset operations
- No direct file I/O in UI code
- Subscribe to index for dropdowns/pickers

### Blueprint Editor Variables Panel
- Query available types from `type_index`
- Present searchable dropdown
- Real-time updates as types are added

### Type Alias Editor
- Use `operations()` for save/create
- Validate names before saving
- Index automatically updates

## File Layout

Type aliases should be stored in:
```
project_root/
  types/
    aliases/
      MyType.alias.json
      Vector3.alias.json
      ...
```

## Thread Safety

- All components use `Arc` for shared ownership
- `DashMap` provides concurrent access without locks
- Safe to use from multiple threads

## Error Handling

All operations return `Result<T>` with context:
```rust
match engine_fs.operations().create_type_alias("MyType", &content) {
    Ok(path) => println!("Created at {:?}", path),
    Err(e) => eprintln!("Failed: {}", e),
}
```

## Future Enhancements

- [ ] Dependency tracking between types
- [ ] Type usage analysis
- [ ] Automatic refactoring support
- [ ] Network synchronization for collaborative editing
- [ ] Undo/redo support at FS level
