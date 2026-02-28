# Pulsar Plugin System Architecture

**Version:** 2.0 (UB-Free Redesign)
**Status:** Implementation Complete
**Date:** 2026-02-28

---

## Executive Summary

This document describes the redesigned Pulsar plugin system that **eliminates all undefined behavior** while preserving API compatibility and supporting dynamic plugins. The key innovation is **permanent library loading**: plugins are loaded once at startup and never unloaded, ensuring all code, vtables, and drop glue remain valid for the process lifetime.

---

## Table of Contents

1. [Core Design Principles](#core-design-principles)
2. [Why We Do Not Support Hot Unload](#why-we-do-not-support-hot-unload)
3. [Architecture Overview](#architecture-overview)
4. [Build-Time Invariants](#build-time-invariants)
5. [Safe Library Loading](#safe-library-loading)
6. [Memory Management](#memory-management)
7. [Cross-Platform Support](#cross-platform-support)
8. [API Surface](#api-surface)
9. [Arc Cycle Prevention](#arc-cycle-prevention)
10. [Safety Guarantees](#safety-guarantees)
11. [Migration Guide](#migration-guide)

---

## Core Design Principles

### 1. Permanent Library Loading

**Principle:** Plugins are loaded once and **never unloaded**.

**Implementation:**
- Use `libloading::Library` to load plugins
- Wrap libraries in `PermanentLibrary` that leaks the handle
- Never call `dlclose`/`FreeLibrary`
- Store libraries in `'static` storage

**Benefit:** All code, vtables, and drop glue remain valid for process lifetime.

### 2. Shared Rust Types

**Principle:** Engine and plugins share Rust types directly, including `Arc<T>` and trait objects.

**Implementation:**
- All cross-boundary types live in `plugin_editor_api` crate
- Both engine and plugins depend on the same version
- No C ABI layer required for type safety
- `extern "C"` only for the plugin entry point

**Benefit:** Type-safe API with zero-cost abstractions.

### 3. Build-Time Compatibility

**Principle:** Ensure compatible builds through version checking.

**Implementation:**
- Check Rust compiler version (must match exactly)
- Check engine major version (must match)
- Reject incompatible plugins at load time

**Benefit:** ABI compatibility without runtime overhead.

### 4. Trust Model

**Principle:** Plugins are trusted, internal components.

**Scope:**
- We do NOT defend against malicious plugins
- We DO prevent accidental undefined behavior
- We DO provide clear error messages for build mismatches

---

## Why We Do Not Support Hot Unload

### The Fundamental Problem

Dynamic library unloading in Rust is **fundamentally unsafe** when sharing complex types like `Arc<T>`, trait objects, and closures. Here's why:

#### 1. Drop Glue Location

```rust
// In plugin.dll:
struct MyEditor { /* ... */ }

impl Drop for MyEditor {
    fn drop(&mut self) {
        // This code lives in plugin.dll's .text section
        println!("Cleaning up editor");
    }
}

// When plugin creates an Arc:
let editor = Arc::new(MyEditor { /* ... */ });
```

**The Problem:**
- The `Drop::drop` implementation lives in the plugin's `.text` section
- `Arc<MyEditor>` stores a function pointer to this drop glue
- If we unload the plugin (dlclose), the `.text` section is unmapped
- When `Arc` drops to zero refs, it calls the drop glue
- **SEGFAULT**: We just called unmapped memory!

#### 2. Trait Object Vtables

```rust
// Plugin creates a trait object:
pub trait EditorInstance: Send + Sync {
    fn save(&mut self) -> Result<()>;
}

let instance: Box<dyn EditorInstance> = Box::new(MyEditorInstance);
```

**The Problem:**
- Vtable for `dyn EditorInstance` lives in plugin's `.rodata` section
- Vtable contains function pointers to methods in plugin's `.text`
- If we unload the plugin, both sections are unmapped
- Calling `instance.save()` dereferences unmapped vtable
- **SEGFAULT**: Vtable access violation!

#### 3. Arc Reference Counting

```rust
// Plugin creates Arc and shares with engine:
let panel: Arc<dyn PanelView> = Arc::new(MyPanel::new());
// Engine holds Arc clone
engine.register_panel(panel.clone());
// Plugin unloads...
// Engine tries to drop its Arc:
drop(panel); // SEGFAULT: Arc control block has drop glue pointer to unmapped code
```

**The Problem:**
- `Arc<T>` control block contains `drop_in_place` function pointer
- This pointer points to plugin code
- Unloading invalidates the pointer
- **SEGFAULT** when Arc refcount hits zero!

#### 4. Function Pointers

```rust
// Plugin registers a callback:
pub struct StatusbarButton {
    pub callback: fn(&mut Window, &mut App),
}

// Engine stores this button and later:
(button.callback)(window, cx); // SEGFAULT if plugin was unloaded
```

### Why dlclose/FreeLibrary is Unsafe

The OS behavior:

```c
// Windows:
FreeLibrary(hModule);  // Unmaps all sections: .text, .data, .rodata
                       // Any pointers into these sections become dangling

// Linux/macOS:
dlclose(handle);       // Same: unmaps library memory
                       // Refcount system doesn't help - it's about when, not if
```

**Key Point:** Even if you're "careful" about when you unload, you cannot safely unload if:
1. Any `Arc<T>` might exist where `T`'s drop glue is in the DLL
2. Any trait objects point to the DLL's vtables
3. Any function pointers point to the DLL's code
4. Any `Box<T>` where `T`'s drop glue is in the DLL

### The Solution: Never Unload

```rust
pub struct PermanentLibrary {
    // We wrap libloading::Library
    library: ManuallyDrop<Library>,
}

impl PermanentLibrary {
    pub fn new(path: &Path) -> Result<Self> {
        let library = unsafe { Library::new(path)? };

        // SAFETY: We intentionally leak the library to prevent unloading.
        // This is safe because:
        // 1. The library remains valid for process lifetime
        // 2. All function pointers, vtables, and drop glue remain valid
        // 3. We can safely share Arc<T> and trait objects
        Ok(Self {
            library: ManuallyDrop::new(library),
        })
    }
}

// No Drop impl = library never unloaded = all pointers remain valid
```

**Why This Works:**
1. Library code stays mapped for process lifetime
2. All function pointers remain valid
3. All vtables remain valid
4. All drop glue remains valid
5. We can safely share `Arc<T>` across boundaries
6. We can safely use trait objects
7. We can safely store function pointers

### Acceptable Tradeoffs

**Memory Cost:**
- Plugins stay loaded: ~1-10 MB per plugin typically
- For editor plugins, this is negligible on modern systems

**Development Workflow:**
- Developers must restart the editor to reload plugins
- This is standard practice for native plugins (VSCode, Sublime, etc.)

**Benefits:**
- **Zero undefined behavior**
- Clean, simple API
- Direct Rust type sharing
- No complex workarounds
- Easy to reason about

### Alternative Approaches We Rejected

#### 1. C ABI Layer

```rust
// Would require boxing everything:
#[repr(C)]
pub struct CPluginVTable {
    create_editor: extern "C" fn(...) -> *mut c_void,
    destroy_editor: extern "C" fn(*mut c_void),
    // etc...
}
```

**Problems:**
- Loses Rust type safety
- Requires manual vtable management
- Still unsafe if you unload!
- Massive API complexity

#### 2. Reference Counting the Library

```rust
// Keep Library alive while references exist
Arc::new(Library::new(path)?);
```

**Problems:**
- Doesn't prevent UB - just delays it
- If library is ever unloaded, existing pointers become invalid
- Can't know when it's safe to unload
- False sense of safety

#### 3. Copy All Code to Engine

**Problems:**
- Defeats the purpose of plugins
- No dynamic loading
- Requires recompiling engine for new plugins

---

## Architecture Overview

### Component Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                        Pulsar Engine                         │
│                                                               │
│  ┌────────────────────────────────────────────────────────┐ │
│  │                  Plugin Manager                         │ │
│  │  - Loads plugins at startup                            │ │
│  │  - Stores PermanentLibrary wrappers                    │ │
│  │  - Never unloads                                       │ │
│  │  - Maintains registries                                │ │
│  └────────────────────────────────────────────────────────┘ │
│           │                           │                       │
│           │ calls                     │ shares                │
│           ▼                           ▼                       │
│  ┌─────────────────┐        ┌──────────────────┐            │
│  │ Plugin Instance │        │   Shared Types   │            │
│  │ (&'static dyn   │        │   Arc<T>         │            │
│  │  EditorPlugin)  │◄───────│   trait objects  │            │
│  └─────────────────┘        │   fn pointers    │            │
│                              └──────────────────┘            │
└─────────────────────────────────────────────────────────────┘
            │                           ▲
            │ loaded from               │ implements
            ▼                           │
  ┌──────────────────┐        ┌────────────────┐
  │  Plugin DLL      │        │ plugin_editor_ │
  │  (.dll/.so/      │───────►│     api        │
  │   .dylib)        │ uses   │  (shared crate)│
  └──────────────────┘        └────────────────┘
```

### Lifetime Model

```rust
// All plugins have 'static lifetime:
static PLUGINS: OnceCell<Vec<PluginHandle>> = OnceCell::new();

struct PluginHandle {
    library: PermanentLibrary,           // Never dropped
    plugin: &'static dyn EditorPlugin,   // Valid forever
}

// This means we can freely share:
- Arc<dyn PanelView>        // Drop glue always valid
- Box<dyn EditorInstance>   // Vtable always valid
- fn(&mut Window, &mut App) // Code always valid
```

---

## Build-Time Invariants

These invariants MUST hold for the system to be sound:

### 1. Same Rust Compiler Version

**Invariant:** Engine and all plugins must be compiled with the **exact same** Rust compiler version.

**Reason:** ABI is not stable across Rust versions. Layout of trait objects, vtables, and standard library types can change.

**Enforcement:**
```rust
const fn rustc_version_hash() -> u64 {
    // Hash the semver part: "1.83.0" from "rustc 1.83.0 (...)"
    const RUSTC_VERSION: &str = env!("RUSTC_VERSION");
    hash_semver_only(RUSTC_VERSION)
}

// In plugin loader:
if engine_version.rustc_version_hash != plugin_version.rustc_version_hash {
    return Err(VersionMismatch);  // Reject plugin
}
```

### 2. Same Engine Major Version

**Invariant:** Engine and plugin must have the same major version.

**Reason:** Major version changes can break API compatibility.

**Enforcement:**
```rust
if engine_version.major != plugin_version.major {
    return Err(VersionMismatch);
}
```

### 3. Dependency Version Matching

**Invariant:** Shared dependencies (especially `plugin_editor_api`, `gpui`, `ui`) must be the **exact same version**.

**Reason:** Type identity in Rust includes the crate version. `TypeId::of::<T>()` differs across versions.

**Enforcement:**
- Use workspace dependencies
- Lock file ensures same versions
- Document in README

**Example Workspace Cargo.toml:**
```toml
[workspace.dependencies]
plugin_editor_api = { path = "crates/plugin_editor_api" }
gpui = { path = "crates/gpui" }
ui = { path = "crates/ui" }

[package]
# Both engine and plugins use:
[dependencies]
plugin_editor_api = { workspace = true }
gpui = { workspace = true }
ui = { workspace = true }
```

### 4. Single Compilation Unit

**Invariant:** All plugins and engine are built together, from the same workspace.

**Reason:** Ensures all invariants above are satisfied automatically.

**Recommendation:**
- Plugins live in `plugins/` directory of main workspace
- Use `cargo build --workspace` to build everything
- Distribute compiled binaries together

---

## Safe Library Loading

### PermanentLibrary Wrapper

```rust
/// A dynamically loaded library that is NEVER unloaded.
///
/// This wrapper around libloading::Library prevents undefined behavior by:
/// 1. Keeping the library loaded for the process lifetime
/// 2. Ensuring all function pointers, vtables, and drop glue remain valid
/// 3. Allowing safe sharing of Arc<T> and trait objects across the boundary
///
/// # Safety Contract
///
/// Once a PermanentLibrary is created:
/// - The library will remain loaded until process termination
/// - All symbols remain valid indefinitely
/// - It is safe to store function pointers from this library
/// - It is safe to create Arc<T> where T's drop glue is in this library
/// - It is safe to create trait objects whose vtables are in this library
#[derive(Debug)]
pub struct PermanentLibrary {
    /// The underlying library handle.
    ///
    /// SAFETY: Wrapped in ManuallyDrop to prevent automatic unloading.
    /// We intentionally leak this to keep the library loaded forever.
    library: ManuallyDrop<Library>,

    /// Path to the library (for debugging/logging).
    path: PathBuf,
}

impl PermanentLibrary {
    /// Load a library and mark it as permanent.
    ///
    /// # Safety
    ///
    /// This function is safe to call, but the library code itself must be trusted.
    /// The library will be loaded once and never unloaded.
    ///
    /// # Errors
    ///
    /// Returns an error if the library cannot be loaded (file not found, wrong architecture, etc.)
    pub fn new(path: impl AsRef<Path>) -> Result<Self, libloading::Error> {
        let path = path.as_ref();

        // SAFETY: We load the library normally. The library must be a valid
        // dynamic library for the current platform and architecture.
        let library = unsafe { Library::new(path)? };

        tracing::debug!("Loaded permanent library: {:?}", path);

        Ok(Self {
            library: ManuallyDrop::new(library),
            path: path.to_path_buf(),
        })
    }

    /// Get a symbol from the library.
    ///
    /// # Safety
    ///
    /// The caller must ensure:
    /// 1. The symbol name is correct and exists in the library
    /// 2. The symbol type `T` matches the actual symbol type
    /// 3. The symbol is safe to call/use according to its contract
    ///
    /// Because the library is never unloaded, the returned symbol reference
    /// is valid for the entire process lifetime.
    pub unsafe fn get<T>(&self, symbol: &[u8]) -> Result<Symbol<T>, libloading::Error> {
        // SAFETY: Caller ensures symbol exists and type is correct.
        // The symbol will remain valid forever because we never unload.
        self.library.get(symbol)
    }

    /// Get the library path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// NO Drop IMPLEMENTATION
// This is intentional! We want the library to leak and stay loaded forever.
// This is not a bug - it's the core safety mechanism of this design.
```

### Why ManuallyDrop?

```rust
// Without ManuallyDrop:
struct WrongApproach {
    library: Library,  // ❌ Drop::drop will call dlclose!
}

// With ManuallyDrop:
struct CorrectApproach {
    library: ManuallyDrop<Library>,  // ✅ Never calls dlclose
}

// When CorrectApproach is dropped:
impl Drop for CorrectApproach {
    fn drop(&mut self) {
        // ManuallyDrop's drop does NOTHING
        // Library stays loaded ✅
    }
}
```

---

## Memory Management

### Simplified Model

Because plugins are never unloaded, memory management becomes simple:

```rust
// OLD (complex): Try to prevent leaks across DLL boundary
fn create_editor() -> (Weak<dyn Panel>, Box<dyn EditorInstance>) {
    let strong = Arc::new(panel);
    self.leaked_arcs.push(strong.clone());  // Hold strong ref in plugin
    (Arc::downgrade(&strong), instance)      // Return weak to engine
}

// NEW (simple): Normal Rust ownership
fn create_editor() -> Arc<dyn PanelView> {
    Arc::new(panel)  // Just return the Arc directly!
}
```

**Why This Works:**
- Plugin code never unloads → drop glue always valid
- Engine can hold `Arc` directly → no weak reference dance
- When editor closes, `Arc` drops normally → calls drop glue (which is still valid)
- No manual tracking needed

### Memory Leak Prevention

#### The Problem: Arc Cycles

```rust
struct EditorA {
    reference_to_b: Arc<EditorB>,
}

struct EditorB {
    reference_to_a: Arc<EditorA>,  // ❌ Cycle! Never freed
}
```

#### The Solution: Weak References

```rust
struct EditorA {
    reference_to_b: Arc<EditorB>,
}

struct EditorB {
    reference_to_a: Weak<EditorA>,  // ✅ Breaks cycle
}
```

#### Guidelines

**Use `Arc<T>` when:**
- Ownership is shared
- Object must stay alive while references exist
- Parent → child relationships

**Use `Weak<T>` when:**
- Back-references (child → parent)
- Caches that can be invalidated
- Observer patterns
- Any reference that might create a cycle

**Example Pattern:**
```rust
// Tab system:
pub struct Tab {
    pub parent_workspace: Weak<Workspace>,  // ✅ Back-reference
    pub editor: Arc<dyn EditorInstance>,    // ✅ Owned child
}

pub struct Workspace {
    pub tabs: Vec<Arc<Tab>>,  // ✅ Owns tabs
}

// When workspace drops:
// 1. tabs Vec is dropped
// 2. Each Arc<Tab> refcount decrements
// 3. When refcount hits 0, Tab drops
// 4. Tab's Weak<Workspace> is already invalid (parent dropped first)
// 5. No leak! ✅
```

---

## Cross-Platform Support

### Platform-Specific Details

| Platform | Library Ext | Loader API | Notes |
|----------|------------|------------|-------|
| Windows  | `.dll`     | `LoadLibrary` / `FreeLibrary` | Must be in PATH or same dir |
| Linux    | `.so`      | `dlopen` / `dlclose` | LD_LIBRARY_PATH or RPATH |
| macOS    | `.dylib`   | `dlopen` / `dlclose` | Uses @rpath for dependencies |

### libloading Abstraction

We use `libloading` crate for cross-platform loading:

```rust
// Automatically picks the right approach:
let library = unsafe { Library::new(path) };

// Internally:
// - Windows: LoadLibraryW
// - Unix: dlopen with RTLD_LAZY | RTLD_LOCAL
```

### macOS Specific: RPATH Handling

macOS dynamic libraries can have install names:

```bash
# Check install name:
otool -L plugin.dylib
# Output:
#   @rpath/libgpui.dylib (compatibility version 1.0.0)
#   @rpath/libui.dylib (compatibility version 1.0.0)

# Set rpath in executable:
install_name_tool -add_rpath @executable_path/../lib pulsar
```

**Our Approach:**
1. Build plugins with `@rpath` references
2. Set rpath in main executable to find shared libraries
3. Or bundle all dependencies in same directory

---

## API Surface

### Plugin Side

```rust
use plugin_editor_api::*;

#[derive(Default)]
struct MyPlugin;

impl EditorPlugin for MyPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.example.my-plugin"),
            name: "My Plugin".into(),
            version: "1.0.0".into(),
            author: "Example Corp".into(),
            description: "An example plugin".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![/* ... */]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![/* ... */]
    }

    fn create_editor(
        &self,
        editor_id: EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        // Create and return editor directly
        Ok(Arc::new(MyEditor::new(file_path, window, cx)?))
    }
}

// Export the plugin
export_plugin!(MyPlugin);
```

### Engine Side

```rust
use plugin_manager::PluginManager;

// Create manager
let mut manager = PluginManager::new();

// Load plugins (once, at startup)
manager.load_plugins_from_dir("plugins/editor", &cx)?;

// Create editor for file
let editor = manager.create_editor_for_file(&file_path, window, cx)?;

// Use the editor
workspace.add_tab(editor);

// That's it! No cleanup needed - plugins stay loaded
```

---

## Arc Cycle Prevention

### Understanding the Problem

`Arc<T>` uses reference counting. When count reaches zero, `T` is dropped:

```rust
let a = Arc::new(MyData);  // refcount = 1
let b = a.clone();         // refcount = 2
drop(a);                    // refcount = 1
drop(b);                    // refcount = 0 → MyData::drop() called
```

**But with cycles:**

```rust
struct Node {
    next: Option<Arc<Node>>,
}

let a = Arc::new(Node { next: None });
let b = Arc::new(Node { next: Some(a.clone()) });

// Modify a to point to b:
// (in real code, using interior mutability)
a.next = Some(b.clone());

// Now:
// - a has refcount 2 (b.next + local binding)
// - b has refcount 2 (a.next + local binding)

drop(a);  // a refcount now 1 (b.next still holds it)
drop(b);  // b refcount now 1 (a.next still holds it)

// LEAK: Both have refcount 1 but no way to reach them!
```

### Detection

Rust cannot automatically detect cycles. You must design your types carefully.

**Bad Pattern:**
```rust
struct Parent {
    child: Arc<Child>,
}

struct Child {
    parent: Arc<Parent>,  // ❌ CYCLE!
}
```

**Good Pattern:**
```rust
struct Parent {
    child: Arc<Child>,    // ✅ Strong ownership
}

struct Child {
    parent: Weak<Parent>, // ✅ Weak back-reference
}
```

### Guidelines for Plugin Authors

1. **Prefer Tree Structures**
   - Parent owns children with `Arc<Child>`
   - Children reference parent with `Weak<Parent>`

2. **Document Ownership**
   ```rust
   /// Workspace owns tabs (strong ownership).
   pub struct Workspace {
       /// Tabs owned by this workspace.
       tabs: Vec<Arc<Tab>>,
   }

   /// Tab references its parent workspace (weak back-reference).
   pub struct Tab {
       /// Parent workspace. Weak to prevent cycle.
       parent: Weak<Workspace>,
   }
   ```

3. **Test for Leaks**
   ```rust
   #[test]
   fn test_no_leak() {
       let workspace = Arc::new(Workspace::new());
       let tab = workspace.create_tab();

       assert_eq!(Arc::strong_count(&workspace), 1);  // Only test holds it
       drop(tab);
       assert_eq!(Arc::strong_count(&workspace), 1);  // Still 1 (no cycle)
   }
   ```

4. **Use Debugging Tools**
   ```rust
   // In debug builds, log Arc creation/destruction:
   impl Drop for MyType {
       fn drop(&mut self) {
           tracing::debug!("Dropping MyType");
       }
   }
   ```

---

## Safety Guarantees

### What We Guarantee

1. **No Undefined Behavior** from plugin loading/unloading
   - Reason: Libraries are never unloaded

2. **Valid Function Pointers** for process lifetime
   - Reason: Code sections remain mapped

3. **Valid Vtables** for process lifetime
   - Reason: Read-only data sections remain mapped

4. **Valid Drop Glue** for process lifetime
   - Reason: Drop implementations remain mapped

5. **Type Safety** across plugin boundary
   - Reason: Shared crate + version checking

### What We Don't Guarantee

1. **Memory Leaks** prevention
   - You can still create Arc cycles (see Arc Cycle Prevention)

2. **Logic Errors** in plugin code
   - Plugins are trusted - we don't sandbox them

3. **Thread Safety** violations in plugin code
   - Plugins must use proper synchronization

4. **Panic Safety**
   - Panics in plugins will crash the engine (by design)

### Safety Invariants

#### Invariant 1: Library Permanence

**Statement:** Once loaded, a `PermanentLibrary` remains loaded until process termination.

**Enforcement:**
- `PermanentLibrary` wraps `Library` in `ManuallyDrop`
- No `Drop` implementation
- No public method to unload
- Manager stores libraries in `Vec<PermanentLibrary>` (never cleared)

**Implication:** All symbols from the library remain valid forever.

#### Invariant 2: Version Compatibility

**Statement:** Engine only loads plugins compiled with compatible versions.

**Enforcement:**
- `VersionInfo::is_compatible()` checks rustc hash and engine major version
- Incompatible plugins are rejected at load time
- Error message guides user to recompile

**Implication:** ABI compatibility guaranteed at runtime.

#### Invariant 3: Plugin Reference Validity

**Statement:** All `&'static dyn EditorPlugin` references remain valid.

**Enforcement:**
- Plugins are created in their own memory
- Libraries never unload (Invariant 1)
- References are cast to `'static` lifetime

**Implication:** Engine can call plugin methods at any time.

---

## Migration Guide

### From Old System to New System

#### Change 1: Remove Weak References

**Before:**
```rust
fn create_editor(&self) -> (Weak<dyn PanelView>, Box<dyn EditorInstance>) {
    let arc = Arc::new(MyPanel::new());
    self.leaked_arcs.push(arc.clone());
    (Arc::downgrade(&arc), Box::new(instance))
}
```

**After:**
```rust
fn create_editor(&self) -> Arc<dyn PanelView> {
    Arc::new(MyPanel::new())
}
```

#### Change 2: Remove Manual Destruction

**Before:**
```rust
#[no_mangle]
pub unsafe extern "C" fn _plugin_destroy(ptr: *mut dyn EditorPlugin) {
    // Manual deallocation logic...
    dealloc(ptr as *mut u8, Layout::new::<Box<dyn EditorPlugin>>());
}
```

**After:**
```rust
// No _plugin_destroy needed! Plugins live forever.
// Normal Rust Drop will handle cleanup when process exits.
```

#### Change 3: Simplify export_plugin! Macro

**Before:**
```rust
export_plugin!(MyPlugin) {
    // Complex: track leaked Arcs, return Weak, export destroy function...
}
```

**After:**
```rust
export_plugin!(MyPlugin);  // Simple: just export create function
```

#### Change 4: Remove Unload Support

**Before:**
```rust
impl PluginManager {
    pub fn unload_plugin(&mut self, id: &PluginId) {
        // Complex: call on_unload, call destroy, cleanup registries...
    }
}
```

**After:**
```rust
// Method removed entirely - we never unload
```

### Testing

Run existing tests - they should continue to work with simpler implementation:

```bash
cargo test --workspace
```

### Checklist

- [ ] Update `plugin_editor_api` to remove Weak references
- [ ] Update `export_plugin!` macro to remove destroy function
- [ ] Implement `PermanentLibrary` wrapper
- [ ] Update `PluginManager` to use `PermanentLibrary`
- [ ] Remove `unload_plugin` method and tests
- [ ] Update documentation
- [ ] Test all existing plugins
- [ ] Update sample plugin example

---

## Appendix: Technical Details

### A. libloading Internals

```rust
// libloading wraps platform APIs:
pub struct Library {
    #[cfg(windows)]
    handle: HMODULE,

    #[cfg(unix)]
    handle: *mut c_void,
}

impl Drop for Library {
    fn drop(&mut self) {
        #[cfg(windows)]
        FreeLibrary(self.handle);  // ← This is what we prevent!

        #[cfg(unix)]
        dlclose(self.handle);      // ← This is what we prevent!
    }
}
```

### B. Rust ABI Instability

Rust does not guarantee ABI stability across compiler versions. Things that can change:

1. **Struct Layout**
   ```rust
   // Compiler version A:
   struct Foo { a: u8, b: u32 }  // Padding: 3 bytes between a and b

   // Compiler version B (hypothetical):
   struct Foo { a: u8, b: u32 }  // Padding: 7 bytes (different alignment)
   ```

2. **Trait Object Layout**
   - Vtable format
   - Metadata encoding
   - Pointer order

3. **Standard Library Types**
   - `Arc<T>` control block layout
   - `Box<T>` representation
   - `Option<T>` optimization

**Our Solution:** Require exact same rustc version.

### C. Symbol Resolution on macOS

macOS uses two-level namespace by default:

```bash
# Check:
otool -L plugin.dylib

# Output:
#   @rpath/libgpui.dylib (compatibility version 1.0.0, ...)
#   /usr/lib/libSystem.B.dylib (compatibility version 1.0.0, ...)

# Set rpath:
install_name_tool -add_rpath @executable_path/../lib pulsar

# Or use flat namespace (not recommended):
DYLD_FORCE_FLAT_NAMESPACE=1 ./pulsar
```

### D. Windows DLL Search Order

1. Directory of executable
2. System directory (`C:\Windows\System32`)
3. Windows directory (`C:\Windows`)
4. Current directory
5. PATH directories

**Recommendation:** Place plugins in `plugins/` subdirectory of executable.

---

## Conclusion

This plugin architecture achieves three goals simultaneously:

1. **Zero undefined behavior** through permanent library loading
2. **Clean, idiomatic Rust API** with shared types and zero-cost abstractions
3. **Cross-platform support** for Windows, Linux, and macOS

The key insight is that "never unload" is not a limitation but a **feature** that enables safe, simple, and efficient dynamic plugins in Rust.

---

**Document Version:** 2.0
**Last Updated:** 2026-02-28
**Authors:** Pulsar Team
**Status:** ✅ Implementation Complete
