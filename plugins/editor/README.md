# Editor Plugins Directory

This directory contains dynamically loaded editor plugins for Pulsar Native.

## Plugin Files

Place compiled plugin DLLs here:
- Windows: `*.dll`
- Linux: `*.so`
- macOS: `*.dylib`

## Currently Available Plugins

(None yet - editors are being migrated from the core engine)

## Planned Plugins

1. **Blueprint Editor** - Visual scripting (.class files)
2. **Script Editor** - Code editing (.rs files)
3. **Type Editors** - Type system editors (.struct, .enum, .trait, .alias)
4. **DAW Editor** - Digital audio workstation (.pdaw files)
5. **Database Editor** - Database viewing/editing (.db files)

## Plugin Development

See the root directory documentation:
- `PLUGIN_SYSTEM_DESIGN.md` - Comprehensive architecture guide
- `PLUGIN_QUICK_REFERENCE.md` - Quick reference for plugin development

## How Plugins Are Loaded

1. On startup, the engine scans this directory for plugin libraries
2. Each plugin is loaded and version-checked
3. File types and editors are automatically registered
4. The "Add File" menu in the file drawer is populated from registered types
5. When a file is opened, the appropriate plugin's editor is instantiated

## Building Plugins

```bash
# Build a plugin
cargo build --release -p <plugin_name>_plugin

# Copy to this directory
cp target/release/lib<plugin_name>_plugin.{dll,so,dylib} plugins/editor/
```

## Plugin Safety

All plugins are version-checked before loading:
- Engine version must match (major version)
- Rust compiler version must match exactly

This prevents ABI incompatibilities and ensures stability.
