# Engine State Resource System

A generic, type-safe, reactive resource system for engine-wide and per-window
state. Lives in `crates/engine_state` — the `StateStore`, `KeyedStore<K>`, and
`ResourceHandle<T>` primitives.

## Motivation

The codebase had ~15 independently-reinvented `OnceLock<RwLock<T>>` /
`Lazy<Mutex<T>>` globals, each with bespoke getter/setter/notify boilerplate.
`EngineContext` itself grew into a 14-field god struct. `EngineBackend` had its
own `static GLOBAL_BACKEND: OnceLock<Arc<RwLock<EngineBackend>>>`.

Instead of adding another named field or another static, store your type in the
generic resource table. No upfront registration needed.

## Core Types

### `ResourceHandle<T>` (`resource.rs`)

A cheap-clone `Arc`-backed handle to a single value of type `T`.

| Method               | Returns                | Behaviour                                                    |
|----------------------|------------------------|--------------------------------------------------------------|
| `.read()`            | `RwLockReadGuard<T>`   | Read-only access (parking_lot, no `.unwrap()`)               |
| `.write()`           | `WriteGuard<T>`        | RAII mutable; bumps version + notifies on drop               |
| `.update(\|t\| ...)` | `R` (closure return)   | Closure-based mutation; bumps version + notifies after       |
| `.set(value)`        | —                      | Replace entire value (shorthand for `update(\|v\| *v = x)`)  |
| `.get() -> T`        | Cloned snapshot        | Requires `T: Clone`                                          |
| `.version() -> u64`  | Monotonic counter      | Cheap "did this change?" check, no lock needed               |
| `.changed()`          | `EventListener<()>`    | Synchronous registration; `.await` later for next change     |

**Key semantic**: `changed()` registers the listener synchronously and returns
a future. The future resolves on the **next** mutation. Multiple independent
callers can each hold their own listener — unlike a single-consumer channel.

```rust
let handle: ResourceHandle<MyState> = /* ... */;
let listener = handle.changed();    // registered NOW
handle.set(new_value);               // triggers notification
listener.await;                      // resolves immediately
```

### `StateStore` (`store.rs`)

A `TypeId`-keyed table holding at most one `ResourceHandle<T>` per type `T`.
Thread-safe, cheaply cloneable (internally `Arc<DashMap<..>>`).

| Method                       | Behaviour                                      |
|------------------------------|-------------------------------------------------|
| `get_or_init::<T: Default>()` | Get handle, creating via `Default` if absent   |
| `insert::<T>(value)`         | Insert/replace, returning a new handle          |
| `get::<T>()`                 | `Option<ResourceHandle<T>>` if already present |
| `contains::<T>()`            | `bool`                                          |
| `remove::<T>()`              | Remove and return handle, if present            |

Use `get_or_init` for the common case (lazy creation on first access).
Use `insert` when you need explicit initialization with non-`Default` values.
Use `get` when you need to distinguish "not yet set" from a default value.

### `KeyedStore<K>` (`keyed_store.rs`)

Same idea as `StateStore`, but keyed by an arbitrary key type `K` (typically
`WindowId`). At most one `ResourceHandle<T>` per `(TypeId, K)` pair.

The API mirrors `StateStore` but takes a key:
`get_or_init::<T>(&key)`, `insert::<T>(key, value)`, `get::<T>(&key)`, etc.

## How to Use

### Adding new engine-wide state

```rust
use engine_state::{EngineContext, ResourceHandle};

#[derive(Default)]
struct GizmoSettings {
    snap_translation: f32,
    snap_rotation: f32,
}

// Anywhere after engine init:
let gizmo: ResourceHandle<GizmoSettings> =
    EngineContext::global().unwrap().store.get_or_init::<GizmoSettings>();

// Read:
let snap = gizmo.read().snap_translation;

// Mutate (closure):
gizmo.update(|g| g.snap_translation = 0.5);

// Mutate (RAII guard):
let mut g = gizmo.write();
g.snap_translation = 0.5;
g.snap_rotation = 15.0;
// guard dropped → version bumped + listeners notified
```

No upfront registration, no `pub static`, no new field on `EngineContext`.

### Adding per-window state

```rust
use engine_state::KeyedStore;
use ui_types_common::window_types::WindowId;

#[derive(Default)]
struct PanelLayout {
    sidebar_width: f32,
}

let layout: ResourceHandle<PanelLayout> =
    EngineContext::global()
        .unwrap()
        .window_state
        .get_or_init::<PanelLayout>(&window_id);
```

### Reacting to changes

```rust
// In a background task / GPUI spawn:
let gizmo = EngineContext::global().unwrap().store.get_or_init::<GizmoSettings>();
loop {
    gizmo.changed().await;
    // Re-read gizmo.read() and update UI
    cx.notify();
}
```

Any number of independent listeners can `await` the same `ResourceHandle` —
each gets its own notification.

## Migration Patterns

### From `static FOO: OnceLock<RwLock<T>>` (std Mutex/RwLock)

**Before:**
```rust
static CACHE: OnceLock<Mutex<SearchCache>> = OnceLock::new();
pub fn global_cache() -> &'static Mutex<SearchCache> {
    CACHE.get_or_init(|| Mutex::new(SearchCache::new()))
}
```
```rust
// Call site — note the .unwrap() / error handling:
if let Ok(mut cache) = global_cache().lock() { cache.insert(k, v); }
```

**After:**
```rust
pub fn global_cache() -> ResourceHandle<SearchCache> {
    EngineContext::global().expect("engine init").store.get_or_init::<SearchCache>()
}
```
```rust
// Call site — infallible:
let cache = global_cache();
cache.write().insert(k, v);
```

Key changes:
- `SearchCache` needs `Default` (or use `insert` at init time)
- No `Mutex`/`OnceLock` imports needed
- `.lock().unwrap()` → `.read()` or `.write()` (infallible parking_lot)
- Bind the `ResourceHandle` to a local before calling `.read()`/`.write()`
  to avoid temporary-lifetime issues:
  ```rust
  // Correct:
  let store = subagent_store();
  let guard = store.read();
  // Wrong (temporary dropped while guard borrows):
  let guard = subagent_store().read(); // COMPILE ERROR
  ```

### From `EngineContext` named fields

**Before:**
```rust
pub struct EngineContext {
    pub project: Arc<RwLock<Option<ProjectContext>>>,
    pub dev: Arc<RwLock<DevContext>>,
    // ... more fields
}
```
```rust
// Call site:
*ctx.dev.write() = dev;
ctx.project.read().as_ref().map(|p| &p.path)
```

**After:**
```rust
// No field on EngineContext — stored in .store:
ctx.store.get_or_init::<DevContext>().set(dev);
ctx.store.get_or_init::<Option<ProjectContext>>().read().as_ref().map(|p| &p.path)
```

For frequently-accessed fields, add a convenience method to `EngineContext`:
```rust
impl EngineContext {
    pub fn set_project(&self, project: ProjectContext) {
        self.store.get_or_init::<Option<ProjectContext>>().set(Some(project));
    }
}
```

### From `static FOO: Lazy<Mutex<T>>` (once_cell)

Same pattern as `OnceLock` above. Remove the `once_cell` dep if it was only
used for this static.

## Gotchas

### Temporary lifetime

`ResourceHandle` is a cheap-clone `Arc` handle. When you call `.read()` or
`.write()`, the guard borrows from the handle. **If the handle is a temporary,
the guard dangles.**

```rust
// WRONG — compile error (E0716):
let guard = subagent_store().read();

// RIGHT:
let store = subagent_store();
let guard = store.read();
```

This also applies to inline calls:
```rust
// WRONG:
global_cache().write().insert(k, v);

// RIGHT:
let cache = global_cache();
cache.write().insert(k, v);
```

### `.update()` vs `.write()`

Prefer `.update(|t| ...)` for new code — it makes the mutation's extent
explicit and is harder to misuse. Use `.write()` (which returns a `WriteGuard`)
when migrating a call site that held a `RwLockWriteGuard` across several
statements.

Both bump the version counter and notify `.changed()` listeners.

### `changed()` is synchronous registration

```rust
handle.changed().await;  // WRONG — rego deferred to first poll
                         // miss notifications between call and .await

let listener = handle.changed();  // rego happens NOW
handle.set(x);                     // listener will be woken
listener.await;                    // resolves immediately
```

### `Option<T>: Default` is always `None`

Types like `Option<ProjectContext>`, `Option<Vec<u8>>` have `Default = None`,
so `get_or_init::<Option<T>>()` returns a handle to `None` without any special
effort. For types that start as `None` and are set later, this is ideal.

## State Already in the Store

| Resource type                    | Set by                  | Read by                |
|----------------------------------|-------------------------|------------------------|
| `Option<MultiuserContext>`       | networking subsystem    | UI status bar          |
| `EngineBackend`                  | engine init             | various subsystems     |
| `ScriptRegistry`                 | script component        | blueprint dispatcher   |
| `RuntimeState`                   | tool context setup      | agent tools            |
| `SubagentStore`                  | agent tools             | agent tools            |
| `TaskManifest`                   | agent tools             | agent tools            |
| `SearchCache`                    | Sketchfab search        | Sketchfab search       |
| `AvatarCache`                    | multiuser UI            | multiuser UI           |
| `Option<ProjectContext>`         | engine init / UI        | many callers           |
| `LaunchContext`                  | engine init             | main.rs                |
| `Option<DiscordPresence>`        | engine init             | Discord RPC            |
| `Option<AuthProfile>`            | auth subsystem          | UI                     |
| `Option<Arc<UserTypeRegistry>>`  | reflection init         | reflection system      |
| `DevContext`                     | engine init             | dev tools / UI         |
| `Option<Vec<u8>>`                | engine init             | level editor           |
| `Option<WindowManager>`          | window system           | window creation        |

## Files

| File                           | Purpose                                              |
|--------------------------------|------------------------------------------------------|
| `crates/engine_state/src/`     |                                                      |
| `resource.rs`                  | `ResourceHandle<T>`, `WriteGuard<T>`                 |
| `store.rs`                     | `StateStore` — type-indexed resource table           |
| `keyed_store.rs`               | `KeyedStore<K>` — per-key resource table             |
| `context.rs`                   | `EngineContext` — thin wrapper holding store         |
| `multiuser.rs`                 | `MultiuserContext` — first field migrated to store   |
| `discord.rs`                   | `DiscordPresence` — migrated into store              |
| `lib.rs`                       | Re-exports, convenience free functions               |
