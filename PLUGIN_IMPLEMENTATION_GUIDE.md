# Plugin Implementation Guide

## Memory-Safe Plugin Pattern

**CRITICAL**: Plugins must manage their own memory to avoid 200-300 MB/sec leaks!

### The Problem

- **DON'T** return `Arc<dyn PanelView>` or `Box<dyn EditorInstance>` - these cause massive leaks
- Shared ownership (Arc) across DLL boundaries = allocator confusion = **MEMORY LEAK**
- Box created in plugin heap, dropped in main app heap = **HEAP CORRUPTION**

### The Solution

**Plugin OWNS all editor instances**. Main app only holds raw pointers.

## Implementation Example

```rust
use plugin_editor_api::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct MyPlugin {
    // Plugin keeps ALL editors alive!
    editors: Arc<Mutex<HashMap<usize, EditorStorage>>>,
    next_id: Arc<Mutex<usize>>,
}

struct EditorStorage {
    panel: Arc<dyn ui::dock::PanelView>,
    instance: Box<dyn EditorInstance>,
}

impl EditorPlugin for MyPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.example.my-plugin"),
            name: "My Plugin".into(),
            version: "1.0.0".into(),
            author: "Me".into(),
            description: "Example plugin".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![/* your file types */]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![/* your editors */]
    }

    fn create_editor(
        &self,
        editor_id: EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(*const dyn ui::dock::PanelView, *mut dyn EditorInstance), PluginError> {
        // Create the actual editor (this is YOUR GPUI element)
        let editor = MyEditor::new(file_path.clone(), window, cx)?;

        // Wrap it for the panel system
        let panel: Arc<dyn ui::dock::PanelView> = Arc::new(editor.clone());
        let instance: Box<dyn EditorInstance> = Box::new(editor);

        // Generate unique ID for this editor
        let id = {
            let mut next = self.next_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };

        // CRITICAL: Plugin keeps Arc and Box alive!
        let panel_ptr = Arc::as_ptr(&panel);
        let instance_ptr = Box::into_raw(instance);

        // Store them in plugin's HashMap
        self.editors.lock().unwrap().insert(id, EditorStorage {
            panel,
            instance: unsafe { Box::from_raw(instance_ptr) }, // Reconstitute for storage
        });

        // Return raw pointers to main app (main app does NOT own these!)
        Ok((panel_ptr, instance_ptr))
    }

    fn destroy_editor(&mut self, editor_instance: *mut dyn EditorInstance) {
        // Find and remove the editor from our storage
        let mut editors = self.editors.lock().unwrap();

        // Find the editor by instance pointer
        editors.retain(|_, storage| {
            let stored_ptr = &*storage.instance as *const dyn EditorInstance;
            stored_ptr != editor_instance as *const _
        });

        // When removed from HashMap, Arc and Box are automatically dropped
        // (in the plugin's heap - SAFE!)
    }

    fn on_load(&mut self) {
        // Plugin initialization
    }

    fn on_unload(&mut self) {
        // Clear all editors
        self.editors.lock().unwrap().clear();
    }
}

// Export the plugin
export_plugin!(MyPlugin);
```

## Key Points

1. **Plugin stores Arc and Box**: Don't let them cross the DLL boundary!
2. **Return raw pointers only**: Main app just holds pointers, doesn't own them
3. **Implement destroy_editor**: Main app calls this when closing a tab
4. **Clear on unload**: Free all editors when plugin unloads

## Main App Integration

When using the plugin manager:

```rust
// Create editor
let (panel_ptr, instance_ptr) = plugin_manager.create_editor_for_file(
    &file_path,
    window,
    cx,
)?;

// Use the pointers (don't drop them!)
let panel: &dyn ui::dock::PanelView = unsafe { &*panel_ptr };

// When closing the tab:
plugin_manager.destroy_editor(&plugin_id, instance_ptr)?;
```

## Why This Works

- Plugin allocates in its heap, frees in its heap ✅
- No Arc/Box crossing DLL boundary ✅
- No allocator mismatch ✅
- No 200-300 MB/sec leak ✅

## Memory Leak Prevention Checklist

- [ ] Plugin stores Arc<dyn PanelView> internally
- [ ] Plugin stores Box<dyn EditorInstance> internally
- [ ] create_editor returns raw pointers only
- [ ] destroy_editor removes from internal storage
- [ ] on_unload clears all editors
- [ ] Main app calls destroy_editor when closing tabs
- [ ] Main app never drops the Arc or Box
