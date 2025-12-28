# Memory Leak Detection Guide

## Overview

The plugin manager now includes comprehensive memory leak detection tools. This guide explains how to use them to identify and fix memory leaks in the plugin system.

## Method 8: Custom Logging Strategy

### 1. Debug State Function

Call `manager.debug_state()` at any point to inspect the current state of the plugin manager:

```rust
use plugin_manager::PluginManager;

let manager = PluginManager::new();
manager.load_plugins_from_dir("plugins/editor", cx)?;

// ... do editor operations ...

// Check state anytime
manager.debug_state();
```

**Output Example:**
```
╔══════════════════════════════════════════════════════════════╗
║          PLUGIN MANAGER STATE (Memory Leak Detection)         ║
╠══════════════════════════════════════════════════════════════╣
║ Loaded Plugins: 2                                            ║
║   ✓ ACTIVE Blueprint Editor - 1 active editors
║      ID: blueprint_editor
║      Version: 0.1.0
║   ✓ ACTIVE Model Editor - 0 active editors
║      ID: model_editor
║      Version: 0.2.0
╠══════════════════════════════════════════════════════════════╣
║ Active Editor Tracking:
║   blueprint_editor has 1 active editor(s)
║ Pending Unload Queue:
║   ✓ (empty - no plugins waiting to be unloaded)
╚══════════════════════════════════════════════════════════════╝
```

### 2. Automatic Shutdown Logging

When the `PluginManager` is dropped (end of program or scope), it automatically logs detailed shutdown information:

```rust
╔══════════════════════════════════════════════════════════════╗
║         PLUGIN MANAGER SHUTDOWN (Memory Cleanup)             ║
╠══════════════════════════════════════════════════════════════╣
║ Unloading 2 plugin(s)...
║ ✓ All editors were properly closed
╠══════════════════════════════════════════════════════════════╣
║ Pending Unloads:
║ ✓ No pending unloads
╠══════════════════════════════════════════════════════════════╣
║ Executing cleanup...
║   ✓ Blueprint Editor
║   ✓ Model Editor
╠══════════════════════════════════════════════════════════════╣
║ Summary:
║   Unloaded: 2/2 plugins
║ ✓ All cleanup complete - memory released
╚══════════════════════════════════════════════════════════════╝
```

## Detecting Memory Leaks

### Red Flags to Watch For:

#### 1. **Lingering Active Editors**
```
║   ⚠️  LEAK DETECTED: blueprint_editor has 2 active editors!
║ ⚠️  Total leaked editor instances: 2
│     (Editors were not properly closed before shutdown)
```

**Action:** Check that `on_editor_closed()` is called when editors are destroyed.

#### 2. **Pending Unloads at Shutdown**
```
║ ⚠️  3 plugins still pending unload:
║      blueprint_editor (forced unload on shutdown)
```

**Action:** These plugins had editors that weren't closed. The system force-unloaded them, which may have caused issues.

#### 3. **Reference Count Issues**
If the same plugin appears in both active_editors and pending_unload, the system is likely handling cleanup correctly, but there may be slow cleanup.

## Best Practices

### 1. **Always Call `on_editor_closed()`**

When an editor is being destroyed:
```rust
// Bad - will cause leaks
fn close_editor(editor_id: EditorId) {
    // ... cleanup ...
    // FORGOT to call on_editor_closed!
}

// Good - properly tracks lifecycle
fn close_editor(editor_id: EditorId, plugin_id: &PluginId) {
    // ... cleanup ...
    manager.on_editor_closed(plugin_id); // ✓ Required!
}
```

### 2. **Monitor State During Testing**

Create a test that opens/closes editors repeatedly:

```rust
#[test]
fn test_editor_lifecycle() {
    let mut manager = PluginManager::new();
    manager.load_plugins_from_dir("plugins/editor", &cx).unwrap();
    
    // Simulate user opening/closing editors
    for i in 0..100 {
        let (panel, editor) = manager
            .create_editor_for_file(&Path::new("test.bp"), &mut window, &mut cx)
            .unwrap();
        
        // Editor is used...
        
        // Close editor
        manager.on_editor_closed(&plugin_id);
        
        // Verify state
        manager.debug_state();
    }
    
    // Check final state
    manager.debug_state();
    // Should show all editors closed and no pending unloads
}
```

### 3. **Periodic State Checks**

Add periodic logging in long-running applications:

```rust
// In your game loop or update function
every_n_frames(1000, |_| {
    manager.debug_state();
});
```

### 4. **Handle Forced Unloads Carefully**

Only use `force_unload_plugin()` in emergency situations:

```rust
// Last resort - only if you're certain no code from the plugin is running
match manager.unload_plugin(&plugin_id) {
    Ok(()) => { /* Normal unload */ }
    Err(e) => {
        log::error!("Failed to unload: {}", e);
        // Only do this if you've verified it's safe
        manager.force_unload_plugin(&plugin_id).ok();
    }
}
```

## Understanding the Lifecycle

### Editor Lifecycle States:

```
┌─────────────────────────────────────────────────┐
│ Plugin Loaded                                   │
│ active_editors[plugin_id] = 0                   │
└──────────────────┬──────────────────────────────┘
                   │
                   ▼
        ┌──────────────────────┐
        │ create_editor() called│
        │ ↓ increment counter    │
        │ active_editors[id] = 1 │
        └──────────────┬────────┘
                       │
                       ▼
            ┌────────────────────┐
            │ Editor Active      │
            │ User edits content │
            └────────────┬───────┘
                         │
                         ▼
              ┌──────────────────────┐
              │ on_editor_closed()   │
              │ ↓ decrement counter   │
              │ active_editors[id]=0  │
              └──────────────┬───────┘
                             │
                ┌────────────┴────────────┐
                │                         │
      (if unload   (if unload
       not pending) pending)
                │                         │
                ▼                         ▼
        ┌──────────────┐        ┌──────────────────┐
        │ Already      │        │ Deferred Unload  │
        │ unloaded!    │        │ Now triggers!    │
        └──────────────┘        └──────────────────┘
```

## Troubleshooting

### Issue: "LEAK DETECTED: blueprint_editor has 3 active editors!"

1. Check all code paths where editors are created
2. Ensure `on_editor_closed()` is called in:
   - Normal close operations
   - Error handling paths
   - Exception handlers
   - Destructors

```rust
// Example: Make sure all paths call on_editor_closed
fn handle_editor(plugin_id: &PluginId) -> Result<()> {
    let (panel, editor) = manager.create_editor_for_file(...)?;
    
    let result = editor.process_file();
    
    // This MUST be called even if process_file() returns error
    manager.on_editor_closed(plugin_id);
    
    result
}
```

### Issue: Plugins stuck in pending unload

This usually means editors haven't been properly closed. Check:
1. Is `on_editor_closed()` being called?
2. Are there circular references preventing cleanup?
3. Are there long-lived Arc references to the editor?

## Integration with CI/CD

Add to your test suite:

```rust
// tests/memory_leak_test.rs
#[test]
fn test_no_memory_leaks() {
    // Run operations that should not leak
    editor_simulation::run_full_simulation();
    
    // Drop manager and capture output
    // (output goes to stderr via eprintln!)
    drop(MANAGER);
    
    // Assert no warnings in output
    // Check logs for "LEAK DETECTED"
}
```

## Performance Note

The `debug_state()` function has minimal overhead and can safely be called frequently during development. The detailed logging only prints to stderr and doesn't affect release builds significantly.

---

**Key Takeaway:** Use `debug_state()` during development to catch leaks early, and the automatic shutdown logging will warn you about any cleanup issues when the program exits.
