# Pulsar Engine Documentation

Welcome to the Pulsar Engine documentation. This guide will help you understand, use, and extend the engine.

## Getting Started

- [Quick Start](../GETTING-A-BLANK-PROJECT.md) - Set up your first project
- [README](../README.md) - Project overview and status

## Architecture

- [Architecture Overview](ARCHITECTURE.md) - System design and structure
- [Type System](TYPE_SYSTEM.md) - Custom type system for game data

## Development

- [Contributing Guide](../CONTRIBUTING.md) - How to contribute
- [Plugin Development](PLUGIN_DEVELOPMENT.md) - Creating editor plugins (includes statusbar buttons)

## Subsystems

### File System
- [File System Design](../crates/engine_fs/README.md) - Asset management

### Multiplayer
- [Multiuser Server](../crates/multiuser_server/README.md) - Collaborative editing

### UI
- [Draggable Tabs](../crates/ui/src/draggable_tabs/README.md) - Tab system
- [UI Compiler](../crates/ui/src/compiler/DESIGN.md) - UI framework

## Examples

- [Type System Demo](../examples/type-system-demo/README.md) - Working with types

## API Documentation

Generate full API docs with:
```bash
cargo doc --open
```

## Project Structure

```
Pulsar-Native/
├── crates/           # Core engine crates
│   ├── engine/       # Runtime engine
│   ├── plugin_*      # Plugin system
│   ├── type_db/      # Type database
│   └── ui/           # UI framework
├── ui-crates/        # Editor UI crates
│   ├── ui_core/      # Main editor
│   ├── ui_file_manager/
│   ├── ui_problems/
│   └── ...
├── docs/             # This documentation
├── examples/         # Example projects
└── plugins/          # Plugin directory
```

## Key Concepts

### Plugins

Plugins extend the editor with new file types and editors. They're compiled as DLLs and loaded dynamically.

See: [Plugin Development](PLUGIN_DEVELOPMENT.md)

### Type System

Pulsar uses a custom type system for strongly-typed game data. Types are defined in Rust and validated at edit time.

See: [Type System](TYPE_SYSTEM.md)

### GPUI

The editor UI uses GPUI, a GPU-accelerated UI framework. UI is declared with a React-like API.

### ECS

The game runtime uses an Entity-Component-System architecture for performance.

## Common Tasks

### Creating a Plugin

1. Create new crate: `cargo new --lib my_plugin`
2. Set `crate-type = ["cdylib"]` in Cargo.toml
3. Implement `EditorPlugin` trait
4. Use `export_plugin!` macro
5. Build and copy to plugins folder

See: [Plugin Development](PLUGIN_DEVELOPMENT.md)

### Adding a File Type

```rust
fn file_types(&self) -> Vec<FileTypeDefinition> {
    vec![
        standalone_file_type(
            "my-type",
            "ext",
            "My Type",
            ui::IconName::FileText,
            gpui::rgb(0x3B82F6),
            serde_json::json!({}),
        )
    ]
}
```

### Adding a Statusbar Button

```rust
fn statusbar_buttons(&self) -> Vec<StatusbarButtonDefinition> {
    vec![
        StatusbarButtonDefinition::new(
            "my-button",
            ui::IconName::Code,
            "My Action",
            StatusbarPosition::Left,
            StatusbarAction::Custom,
        )
        .with_callback(my_callback)
    ]
}
```

## Troubleshooting

### Build Errors

- Ensure Rust toolchain matches (check `rust-toolchain` file)
- Run `cargo clean` and rebuild
- Check for missing dependencies

### Plugin Not Loading

- Verify same Rust version as engine
- Check plugin in correct directory
- Look for errors in console output

### Editor Performance

- Profile with release builds
- Check FPS in debug overlay
- Review asset loading patterns

## Getting Help

- **Discord**: Join our community server
- **GitHub Issues**: Report bugs
- **Discussions**: Ask questions

## Contributing

We welcome contributions! See [Contributing Guide](../CONTRIBUTING.md) for:

- Code style guidelines
- Pull request process
- Testing requirements
- AI assistance policy

## License

See [LICENSE.md](../LICENSE.md) for details.

## Roadmap

Track progress on our [GitHub Projects](https://github.com/orgs/Far-Beyond-Pulsar/projects/1) board.

Major upcoming features:

- Cross-platform support restoration
- Rendering pipeline overhaul
- Asset streaming system
- Improved multiplayer
- Plugin marketplace

---

*Documentation updated: 2026-01-04*
