# Plugin Development Guide

This guide covers creating editor plugins for Pulsar Engine. Plugins can add custom file types, editors, and statusbar buttons.

## Table of Contents

- [Quick Start](#quick-start)
- [Plugin Structure](#plugin-structure)
- [File Type Registration](#file-type-registration)
- [Editor Implementation](#editor-implementation)
- [Statusbar Buttons](#statusbar-buttons)
- [Building and Testing](#building-and-testing)
- [Best Practices](#best-practices)
- [Troubleshooting](#troubleshooting)

## Quick Start

### 1. Create Plugin Crate

```bash
cargo new --lib my_editor_plugin
cd my_editor_plugin
```

### 2. Configure Cargo.toml

```toml
[package]
name = "my_editor_plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]  # Required for dynamic loading

[dependencies]
plugin_editor_api = { path = "../Pulsar-Native/crates/plugin_editor_api" }
gpui = { path = "../Pulsar-Native/crates/gpui" }
ui = { path = "../Pulsar-Native/crates/ui" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### 3. Implement Plugin

```rust
use plugin_editor_api::*;
use std::path::PathBuf;

#[derive(Default)]
struct MyEditorPlugin;

impl EditorPlugin for MyEditorPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.example.my-editor"),
            name: "My Editor".into(),
            version: "0.1.0".into(),
            author: "Your Name".into(),
            description: "Custom file editor".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            standalone_file_type(
                "my-file",
                "myfile",
                "My File",
                ui::IconName::FileText,
                gpui::rgb(0x3B82F6),
                serde_json::json!({"version": 1}),
            )
        ]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("my-editor"),
                display_name: "My Editor".into(),
                supported_file_types: vec![FileTypeId::new("my-file")],
            }
        ]
    }

    fn create_editor(
        &self,
        _editor_id: EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
        _logger: &EditorLogger,
    ) -> Result<(std::sync::Weak<dyn ui::dock::PanelView>, Box<dyn EditorInstance>), PluginError> {
        let editor = MyEditor::new(file_path, window, cx)?;

        // Create Arc and store it in plugin-owned storage to prevent memory leaks
        let panel = Arc::new(editor.panel_wrapper());

        // IMPORTANT: Store the strong Arc in your plugin's state
        // (Implementation depends on your plugin's architecture)
        // self.panels.push(Arc::clone(&panel));

        // Return Weak reference to prevent Arc leaks across DLL boundary
        let weak_panel = Arc::downgrade(&panel);
        let instance = Box::new(editor);

        Ok((weak_panel, instance))
    }
}

// Export the plugin
export_plugin!(MyEditorPlugin);
```

### 4. Build and Install

```bash
cargo build --release
cp target/release/my_editor_plugin.dll /path/to/Pulsar/plugins/editor/
```

## Plugin Structure

### Required Components

Every plugin must implement:

1. **EditorPlugin trait**: Main plugin interface
2. **Metadata**: Plugin identification
3. **File types**: Asset types the plugin handles
4. **Editors**: Editor implementations
5. **export_plugin! macro**: FFI export

### Optional Components

- **Statusbar buttons**: Quick access actions
- **on_load/on_unload hooks**: Lifecycle callbacks

## File Type Registration

### Standalone Files

Single file assets (e.g., `.txt`, `.json`):

```rust
fn file_types(&self) -> Vec<FileTypeDefinition> {
    vec![
        standalone_file_type(
            "my-data",           // Unique ID
            "data",              // File extension
            "Data File",         // Display name
            ui::IconName::Database,
            gpui::rgb(0x10B981), // Icon color
            serde_json::json!({  // Default content
                "version": 1,
                "data": []
            }),
        )
    ]
}
```

### Folder-Based Files

Folder treated as single asset (e.g., `.class/`):

```rust
fn file_types(&self) -> Vec<FileTypeDefinition> {
    vec![
        folder_file_type(
            "my-complex-asset",
            "asset",                    // Folder extension
            "Complex Asset",
            ui::IconName::Folder,
            gpui::rgb(0x8B5CF6),
            "manifest.json",            // Marker file
            vec![                       // Template structure
                PathTemplate::File {
                    path: "manifest.json".into(),
                    content: r#"{"name": "New Asset"}"#.into(),
                },
                PathTemplate::Folder {
                    path: "data".into(),
                },
            ],
            serde_json::json!({"name": ""}),
        )
    ]
}
```

### With Categories

Organize in creation menus:

```rust
let mut file_type = standalone_file_type(/* ... */);
file_type.categories = vec!["Data".into(), "Config".into()];
// Appears in: Create > Data > Config > My File
```

## Editor Implementation

### Basic Editor Structure

```rust
use gpui::*;
use ui::dock::*;

struct MyEditor {
    file_path: PathBuf,
    content: String,
    dirty: bool,
}

impl MyEditor {
    fn new(file_path: PathBuf, _window: &mut Window, _cx: &mut App) 
        -> Result<Self, PluginError> 
    {
        let content = std::fs::read_to_string(&file_path)
            .map_err(|e| PluginError::FileLoadError {
                path: file_path.clone(),
                message: e.to_string(),
            })?;

        Ok(Self {
            file_path,
            content,
            dirty: false,
        })
    }

    fn panel_wrapper(self) -> impl PanelView {
        // Create panel wrapper for tab system
        // See example plugins for full implementation
    }
}

impl EditorInstance for MyEditor {
    fn file_path(&self) -> &PathBuf {
        &self.file_path
    }

    fn save(&mut self, _window: &mut Window, _cx: &mut App) 
        -> Result<(), PluginError> 
    {
        std::fs::write(&self.file_path, &self.content)
            .map_err(|e| PluginError::FileSaveError {
                path: self.file_path.clone(),
                message: e.to_string(),
            })?;
        self.dirty = false;
        Ok(())
    }

    fn reload(&mut self, _window: &mut Window, _cx: &mut App) 
        -> Result<(), PluginError> 
    {
        self.content = std::fs::read_to_string(&self.file_path)
            .map_err(|e| PluginError::FileLoadError {
                path: self.file_path.clone(),
                message: e.to_string(),
            })?;
        self.dirty = false;
        Ok(())
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
```

### Rendering UI

Editors use GPUI for rendering:

```rust
impl Render for MyEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) 
        -> impl IntoElement 
    {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                div()
                    .p_4()
                    .text_color(cx.theme().foreground)
                    .child(self.content.clone())
            )
    }
}
```

See [UI Development](UI_DEVELOPMENT.md) for GPUI details.

## Statusbar Buttons

Plugins can add buttons to the editor's statusbar for quick access to features.

**Important Memory Safety Note:** Button data (including strings and function pointers) is automatically deep-cloned into the main app's heap when registered. This prevents memory corruption when your plugin is unloaded. All buttons are automatically removed when your plugin unloads.

### Basic Usage

Implement the `statusbar_buttons()` method:

```rust
fn statusbar_buttons(&self) -> Vec<StatusbarButtonDefinition> {
    vec![
        StatusbarButtonDefinition::new(
            "my-plugin.toggle-panel",
            ui::IconName::Code,
            "Toggle Panel",
            StatusbarPosition::Left,
            StatusbarAction::ToggleDrawer {
                drawer_id: "my-panel".into(),
            },
        )
    ]
}
```

### Button Definition

Required parameters for `StatusbarButtonDefinition::new()`:

- `id` - Unique identifier (e.g., "my-plugin.action")
- `icon` - Icon from `ui::IconName`
- `tooltip` - Text shown on hover
- `position` - `StatusbarPosition::Left` or `::Right`
- `action` - What happens on click

### Positioning

Choose where buttons appear:

- `StatusbarPosition::Left` - With drawer toggles (Files, Problems, etc.)
- `StatusbarPosition::Right` - With status indicators (before project name)

### Actions

Three action types available:

**Toggle a Drawer**
```rust
StatusbarAction::ToggleDrawer {
    drawer_id: "my-drawer".into(),
}
```

**Open an Editor**
```rust
StatusbarAction::OpenEditor {
    editor_id: EditorId::new("my-editor"),
    file_path: Some(PathBuf::from("config.json")),
}
```

**Run Custom Code**
```rust
StatusbarAction::Custom
// Must also call: .with_callback(my_function)

fn my_function(_window: &mut Window, _cx: &mut App) {
    // Your code here
}
```

### Optional Features

**Badge Count** - Display notification count:
```rust
.with_badge(5)  // Shows "5" in circle
```

**Badge Color** - Customize badge:
```rust
.with_badge_color(gpui::rgb(0xF44336))  // Red
```

**Priority** - Control order (higher = first):
```rust
.with_priority(200)  // High priority
.with_priority(100)  // Normal priority
```

**Active State** - Highlight button:
```rust
.with_active(true)  // Blue background tint
```

**Icon Color** - Override default:
```rust
.with_icon_color(gpui::rgb(0x3B82F6))  // Blue
```

### Complete Example

```rust
fn statusbar_buttons(&self) -> Vec<StatusbarButtonDefinition> {
    let error_count = self.get_error_count();
    let panel_open = self.is_panel_open();
    
    vec![
        StatusbarButtonDefinition::new(
            "my-plugin.errors",
            ui::IconName::TriangleAlert,
            format!("{} Errors", error_count),
            StatusbarPosition::Left,
            StatusbarAction::ToggleDrawer {
                drawer_id: "my-errors".into(),
            },
        )
        .with_priority(150)
        .with_badge(error_count)
        .with_badge_color(gpui::rgb(0xF44336))
        .with_active(panel_open),
    ]
}
```

### Multiple Buttons

Return multiple buttons in the vector:

```rust
fn statusbar_buttons(&self) -> Vec<StatusbarButtonDefinition> {
    vec![
        // Main action
        StatusbarButtonDefinition::new(
            "my-plugin.main",
            ui::IconName::Play,
            "Run",
            StatusbarPosition::Right,
            StatusbarAction::Custom,
        )
        .with_priority(200)
        .with_callback(run_action),
        
        // Settings
        StatusbarButtonDefinition::new(
            "my-plugin.settings",
            ui::IconName::Settings,
            "Settings",
            StatusbarPosition::Right,
            StatusbarAction::Custom,
        )
        .with_priority(100)
        .with_callback(open_settings),
    ]
}
```

### Custom Callbacks

For `StatusbarAction::Custom`, provide a function pointer:

```rust
fn my_callback(_window: &mut Window, cx: &mut App) {
    tracing::info!("Button clicked!");
    // Can show notifications, open windows, modify state, etc.
}

// Register with:
.with_callback(my_callback)
```

Note: Must be a function pointer, not a closure.

### Styling Guide

**Icon Choices:**
- File operations: `Folder`, `FileText`, `FilePlus`
- Actions: `Play`, `Pause`, `Stop`, `Refresh`
- Indicators: `CheckCircle`, `AlertTriangle`, `Info`
- Tools: `Settings`, `Search`, `Terminal`

**Badge Colors:**
```rust
// Errors
.with_badge_color(gpui::rgb(0xF44336))  // Red

// Warnings
.with_badge_color(gpui::rgb(0xFF9800))  // Orange

// Info
.with_badge_color(gpui::rgb(0x2196F3))  // Blue

// Success
.with_badge_color(gpui::rgb(0x4CAF50))  // Green
```

**Icon Colors:**

Use theme colors when possible:
```rust
.with_icon_color(cx.theme().muted_foreground)  // Gray
.with_icon_color(cx.theme().accent)            // Blue
.with_icon_color(cx.theme().danger)            // Red
```

**Tooltips:**

Write clear, concise tooltips:
- Good: "Toggle Error Panel", "Run Tests", "Open Settings"
- Avoid: "Click this button...", "Button", ""

### Priority Guidelines

Suggested ranges:
- **200-300**: Critical actions, main features
- **100-199**: Standard actions
- **0-99**: Secondary actions, settings

Higher priority appears first (leftmost) within its position group.

## Building and Testing

### Development Build

```bash
cargo build
cp target/debug/my_plugin.dll %AppData%/Pulsar/plugins/editor/
```

### Release Build

```bash
cargo build --release
cp target/release/my_plugin.dll %AppData%/Pulsar/plugins/editor/
```

### Testing

1. Launch Pulsar Engine
2. Check console for plugin load messages
3. Create a new file with your custom type
4. Verify editor opens correctly

### Logging

Use `tracing` macros for debugging:

```rust
use tracing::{info, warn, error, debug};

info!("Plugin loaded successfully");
debug!("File opened: {:?}", path);
error!("Failed to parse: {}", err);
```

## Best Practices

### File Handling

- **Always validate** file content before parsing
- **Handle errors gracefully** - show error UI instead of panicking
- **Use background threads** for heavy I/O
- **Track dirty state** accurately

### UI Design

- **Follow theme colors** - use `cx.theme().*`
- **Responsive layout** - adapt to window size
- **Keyboard shortcuts** - support common bindings
- **Accessibility** - clear focus indicators

### Performance

- **Lazy load** content when possible
- **Virtualize** large lists
- **Debounce** rapid changes
- **Profile** with release builds

### Memory Management

**Critical: DLL Boundary Memory Safety**

When returning `Arc` across DLL boundaries, reference counts can leak. The plugin API uses `Weak` references to prevent this:

```rust
// Plugin must store strong Arcs internally
struct MyPlugin {
    panels: Vec<Arc<dyn PanelView>>,  // Plugin owns these
}

fn create_editor(...) -> Result<(Weak<dyn PanelView>, ...), ...> {
    let panel = Arc::new(my_panel);

    // Store strong reference in plugin
    self.panels.push(Arc::clone(&panel));

    // Return weak reference to main app
    Ok((Arc::downgrade(&panel), editor_instance))
}
```

The main app upgrades the `Weak` when needed. When your plugin unloads, all strong `Arc`s are dropped, invalidating the weak references and preventing leaks.

**Best Practices:**

- **Clean up** in `on_unload()` - clear your panel storage
- **Use Arc** for shared data within your plugin
- **Return Weak** from `create_editor()` (required by API)
- **Store strong Arcs** in your plugin struct
- **Avoid cycles** in reference counting
- **Test for leaks** with long sessions

### Error Handling

```rust
// Good: Specific error with context
Err(PluginError::InvalidFormat {
    expected: "JSON".into(),
    message: format!("Line {}: {}", line, err),
})

// Bad: Generic error
Err(PluginError::Other {
    message: "Error".into(),
})
```

## Troubleshooting

### Plugin Not Loading

**Check logs** for error messages:
- Wrong version (ABI mismatch)
- Missing symbols
- Initialization failure

**Verify:**
- Plugin in correct directory
- Built with same Rust version as engine
- `export_plugin!` macro used

### Editor Not Opening

**Causes:**
- File type not registered
- No editor for file type
- Editor creation error

**Debug:**
- Check `file_types()` returns definitions
- Verify `create_editor()` succeeds
- Look for error messages in console

### Crashes

**Common issues:**
- Null pointer dereference
- Thread safety violation
- Panic in plugin code

**Solutions:**
- Use `Result` for fallible operations
- Test with debug builds first
- Add logging around unsafe code

### Performance Issues

**Profile to identify:**
- Excessive rendering
- Heavy computations on main thread
- Memory allocations

**Optimize:**
- Move work to background threads
- Cache computed values
- Use efficient data structures

## Example Plugins

Reference implementations:

- **editor_script**: Rust source editor
- **editor_toml**: TOML configuration editor
- **editor_markdown**: Markdown viewer
- **editor_sqlite**: Database browser

Located in `Pulsar-Native/crates/`.

## Version Compatibility

Plugins must match the engine's:

1. **Engine version**: Major.minor must match
2. **Rust version**: Exact compiler version
3. **ABI hash**: Ensures binary compatibility

The plugin system enforces these at load time.

## Publishing Plugins

(Coming soon)

- Plugin repository
- Automatic updates
- Version management
- User reviews

## Advanced Topics

### Multi-File Editing

Handle multiple files in one editor:

```rust
struct ComplexEditor {
    main_file: PathBuf,
    related_files: Vec<PathBuf>,
}
```

### Background Processing

Long operations without blocking UI:

```rust
cx.spawn(async move |editor, mut cx| {
    let result = heavy_computation().await;
    editor.update(&mut cx, |editor, cx| {
        editor.apply_result(result);
        cx.notify();
    });
})
```

### Custom Themes

Support user themes:

```rust
.text_color(cx.theme().foreground)
.bg(cx.theme().background)
.border_color(cx.theme().border)
```

### Undo/Redo

Implement command pattern:

```rust
trait Command {
    fn execute(&mut self, editor: &mut MyEditor);
    fn undo(&mut self, editor: &mut MyEditor);
}
```

## API Reference

Full API documentation:

- [plugin_editor_api docs](https://docs.rs/plugin_editor_api)
- [EditorPlugin trait](../crates/plugin_editor_api/src/lib.rs)
- [GPUI documentation](https://docs.rs/gpui)

## Getting Help

- **Discord**: Real-time chat with developers
- **GitHub Issues**: Bug reports and feature requests
- **Discussions**: Design questions and proposals

## Contributing

Plugin improvements welcome! See [Contributing Guide](../CONTRIBUTING.md).

---

**Next Steps:**
- Review example plugins in `/crates`
- Check [UI Development](UI_DEVELOPMENT.md) for GPUI details
- Read [Architecture](ARCHITECTURE.md) for system overview
