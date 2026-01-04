# Pulsar Engine Architecture

This document outlines the high-level architecture of Pulsar Engine, covering the core systems, their responsibilities, and how they interact.

## Overview

Pulsar is structured as a modular game engine with clear separation between engine services, UI systems, and game content. The architecture prioritizes:

- **Modularity**: Systems are loosely coupled through well-defined interfaces
- **Extensibility**: Plugin system allows adding editors and file types
- **Type Safety**: Rust's type system enforces correctness at compile time
- **Performance**: Native code with zero-cost abstractions

## System Layers

```
┌─────────────────────────────────────────────────────────┐
│                    UI Layer (GPUI)                      │
│  Entry Point │ Launcher │ Settings │ Editor Windows     │
└─────────────────────────────────────────────────────────┘
                            │
┌─────────────────────────────────────────────────────────┐
│                  Editor Services                        │
│  File Manager │ Problems │ Terminal │ Type Debugger     │
└─────────────────────────────────────────────────────────┘
                            │
┌─────────────────────────────────────────────────────────┐
│                  Plugin System                          │
│  Editor Plugins │ File Type Registry │ Custom Editors   │
└─────────────────────────────────────────────────────────┘
                            │
┌─────────────────────────────────────────────────────────┐
│                  Engine Backend                         │
│  Rust Analyzer │ Type Database │ File System Services   │
└─────────────────────────────────────────────────────────┘
                            │
┌─────────────────────────────────────────────────────────┐
│                  Core Engine                            │
│  Rendering │ ECS │ Physics │ Asset Management           │
└─────────────────────────────────────────────────────────┘
```

## Core Components

### UI Layer (`ui-crates/`)

The UI layer provides the editor interface and tools.

**Key Crates:**
- `ui_core` - Main editor application, window management
- `ui_entry` - Entry screen for selecting projects
- `ui_project_launcher` - Project selection and creation
- `ui_settings` - Engine and project settings UI
- `ui_file_manager` - Project file browser and navigation
- `ui_problems` - LSP diagnostic viewer
- `ui_terminal` - Integrated terminal
- `ui_type_debugger` - Type system inspection tools
- `ui_level_editor` - 3D scene editor
- `ui_multiplayer` - Collaborative editing features

**UI Framework:**

Pulsar uses GPUI (a GPU-accelerated UI framework) for all editor UI. Key concepts:

- **Entities**: Managed UI state containers
- **Components**: Reusable UI elements
- **Context**: Threading and state management
- **Rendering**: Declarative element trees

### Plugin System (`crates/plugin_manager`, `crates/plugin_editor_api`)

The plugin system enables dynamic loading of editor extensions.

**Responsibilities:**
- Load/unload editor plugins from DLLs
- Version compatibility checking
- File type and editor registration
- Statusbar button registration
- Cross-platform ABI handling

**Key Types:**
- `EditorPlugin` - Main plugin trait
- `FileTypeDefinition` - Custom file type specs
- `EditorMetadata` - Editor capabilities
- `EditorInstance` - Runtime editor state

Plugins are compiled as dynamic libraries and loaded at runtime. See [Plugin Development Guide](PLUGIN_DEVELOPMENT.md) for details.

### Engine Backend (`crates/engine_backend`)

Backend services that support the editor experience.

**Services:**
- **Rust Analyzer Manager**: LSP integration for Rust code
- **Type Database**: Project type information and queries
- **File System Services**: File watching and change detection

These services run on background threads and communicate with the UI through async channels.

### Type System (`crates/type_db`, `crates/pulsar_std`)

Pulsar has a custom type system for game data.

**Components:**
- **Type Database**: Stores all project types (structs, enums, traits, aliases)
- **Type Registry**: Runtime type lookup and validation
- **Standard Library**: Built-in types and utilities

The type system enables:
- Strongly typed game data files
- Editor validation and autocomplete
- Runtime type introspection
- Cross-file type references

### File System (`crates/engine_fs`)

The file system layer manages project assets and structure.

**Features:**
- Folder-based file types (e.g., `.class` folders)
- Template expansion for new assets
- Nested folder support
- Category-based organization

See [File System Design](engine_fs/README.md) for implementation details.

### Core Engine (`crates/engine`)

The runtime game engine (3D rendering, ECS, physics, etc.).

**Major Systems:**
- Rendering pipeline (Vulkan/Metal/DirectX)
- Entity-Component-System (ECS)
- Physics simulation
- Asset loading and streaming
- Audio system

## Data Flow

### Opening a Project

1. User selects project in launcher
2. `ui_core` initializes with project path
3. File manager scans project structure
4. Plugins load and register file types
5. Rust Analyzer starts indexing
6. Type Database loads project types
7. Editor windows become available

### Editing a File

1. User double-clicks file in file manager
2. Plugin manager finds suitable editor
3. Editor plugin creates editor instance
4. File content loads into editor
5. Changes trigger file watching events
6. Rust Analyzer updates diagnostics
7. Problems panel displays issues

### Building the Project

1. User triggers build command
2. Terminal executes `cargo build`
3. Rust Analyzer receives build output
4. Diagnostics parse and display
5. Type Database updates on success

## Threading Model

Pulsar uses a hybrid threading approach:

**Main Thread:**
- UI rendering (GPUI)
- User input handling
- Editor state updates

**Background Threads:**
- Rust Analyzer (LSP)
- File system watching
- Type Database queries
- Build processes

**Async Tasks:**
- Network requests (multiplayer)
- Long-running operations
- Plugin operations

Communication between threads uses:
- `tokio` async runtime
- GPUI's `Context` for UI updates
- `Arc<Mutex<T>>` for shared state
- Message passing channels

## Memory Management

**Key Principles:**
- GPUI `Entity<T>` for UI state lifecycle
- Rust ownership prevents memory leaks
- Plugins use raw pointers (FFI safety)
- Explicit `Arc` for shared ownership

**Plugin Memory:**

Plugins allocate memory in their own DLL heap. The main app never calls `drop` on plugin-owned objects. Instead, plugins provide `_plugin_destroy` functions to free memory in their heap.

## File Formats

### Text-Based Assets

Most assets are text for Git-friendly versioning:

- **Scripts**: `.rs` Rust source files
- **Types**: Rust type definitions
- **Data**: JSON/TOML configuration files
- **Scenes**: Custom text scene format

### Binary Assets

Some assets remain binary:

- **Textures**: PNG, JPG, etc.
- **Models**: glTF, FBX (converted to internal format)
- **Audio**: WAV, OGG, MP3
- **Shaders**: Compiled SPIR-V

## Configuration

### Engine Configuration

Located in `%AppData%/Pulsar/`:

- `themes/` - UI theme files
- `config.json` - Global engine settings
- `plugins/` - User-installed plugins

### Project Configuration

Located in project root:

- `Cargo.toml` - Rust project manifest
- `.pulsar/` - Engine-specific project data
- `src/` - Game source code
- `assets/` - Game assets

## Extension Points

The engine provides several extension mechanisms:

1. **Editor Plugins**: Custom file editors
2. **File Types**: New asset types
3. **UI Components**: Reusable widgets
4. **Build Tools**: Custom build steps
5. **Statusbar Buttons**: Quick actions

See individual guides for each extension point.

## Security Considerations

**Plugin Safety:**

Plugins run in the same process as the main app. They have full system access. Only load plugins from trusted sources.

**Future Work:**

- WebAssembly plugin sandbox
- Permission system for plugins
- Code signing for verified plugins

## Performance Characteristics

**Editor Startup:**
- Cold start: 2-5 seconds
- Project loading: 1-3 seconds
- Plugin loading: <100ms per plugin

**Runtime Performance:**
- UI rendering: 60+ FPS
- File operations: Async, non-blocking
- LSP responses: <100ms typical

**Memory Usage:**
- Base editor: ~200MB
- Per plugin: ~10-50MB
- Per editor instance: ~5-20MB

## Build System

Pulsar uses Cargo (Rust's build tool):

```
Cargo.toml (workspace)
├── crates/
│   ├── engine/
│   ├── type_db/
│   └── ...
└── ui-crates/
    ├── ui_core/
    ├── ui_file_manager/
    └── ...
```

Dependencies are managed through Cargo and compiled together. The entire editor is a single Rust workspace.

## Related Documentation

- [Plugin Development Guide](PLUGIN_DEVELOPMENT.md)
- [File System Design](engine_fs/README.md)
- [Type System Reference](TYPE_SYSTEM.md)
- [UI Development](UI_DEVELOPMENT.md)
- [Contributing Guidelines](../CONTRIBUTING.md)

## Future Architecture Changes

**Planned Improvements:**

1. **Rendering Pipeline Overhaul**: Modern Vulkan/DX12 renderer
2. **ECS Redesign**: Performance-focused entity system
3. **Asset Pipeline**: Streaming and LOD system
4. **Network Layer**: Multiplayer infrastructure
5. **Script Hot-Reload**: Live code updates

These changes will maintain the plugin architecture while improving core systems.

