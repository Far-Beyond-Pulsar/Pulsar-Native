# Subsystems Architecture

Components get runtime context through a single trait with three methods:
`subsystems_mut()`, `project_root()`, and `report_error()`. All domain-specific
services — renderer, mesh cache, physics engine, etc. — are accessed through
a type-erased `Subsystems` registry keyed by `TypeId`.

## Design Goal

Central systems (editor, game loader) must not know what any component does.
Each component is fully self-contained: it parses its own data, loads assets,
and writes to subsystems through the registry.

## Layers

```
                    ┌────────────────────────────────┐
                    │  pulsar_reflection             │
                    │  ┌───────────────────────────┐ │
                    │  │ Subsystems                │ │
                    │  │  - owned: HashMap<TypeId, │ │
                    │  │           Box<dyn Any>>   │ │
                    │  │  - borrow: HashMap<TypeId,│ │
                    │  │             *mut ()>      │ │
                    │  └───────────────────────────┘ │
                    │  LiveKeySet                    │
                    │  ComponentRuntimeContext trait │
                    │  get_subsystem! macro          │
                    └────────────────────────────────┘
                              │
                              ▼
                    ┌────────────────────────────────┐
                    │  pulsar_rendering::subsystems  │
                    │  ┌─────────────────────────┐   │
                    │  │ MeshCache               │   │
                    │  │ SceneObjectCache        │   │
                    │  │ resolve_asset_path()    │   │
                    │  │ load_mesh_upload()      │   │
                    │  └─────────────────────────┘   │
                    └────────────────────────────────┘
                              │
                    ┌─────────┴──────────┐
                    ▼                    ▼
           ┌───────────────┐   ┌──────────────────┐
           │ Components    │   │ Central Systems  │
           │ (in crate     │   │ (engine_backend, │
           │  pulsar_)     │   │  pulsar_scene)   │
           │ rendering     │   │                  │
           │               │   │ Only import from │
           │ Access via    │   │ subsystems::     │
           │ get_subsystem!│   │                  │
           └───────────────┘   └──────────────────┘
```

## Subsystems Registry (`pulsar_reflection`)

The `Subsystems` struct stores values by `TypeId` in two maps:

| Storage   | Method            | Lifetime         |
|-----------|-------------------|------------------|
| Owned     | `register()`      | Registry-bound   |
| Borrowed  | `register_ref()`  | Caller-guaranteed|

`register_ref` takes a `&mut T` and stores a raw pointer. It is used for
subsystems that live in outer structs (e.g., the helio `Renderer` inside
`HelioInner`).

## `get_subsystem!` Macro

```rust
let renderer = get_subsystem!(context, helio::Renderer);
```

Expands to a mutable borrow of `context.subsystems_mut()` followed by a
`downcast_mut` — panics with the subsystem name if unregistered.

Because it borrows context mutably, you cannot hold two subsystem references
at the same time. Use sequential scopes:

```rust
let (mesh_id, mat_id) = {
    let mc = get_subsystem!(context, MeshCache);
    mc.get(&abs_path).unwrap()
};
let renderer = get_subsystem!(context, Renderer);
renderer.scene_mut().insert_actor(actor);
```

## Central Systems

Central systems register subsystems before the component sync pass:

```rust
let mut subsystems = Subsystems::new();
subsystems.register_ref::<Renderer>(&mut inner.renderer);
subsystems.register_ref::<MeshCache>(&mut inner.mesh_cache);
subsystems.register_ref::<SceneObjectCache>(&mut inner.object_cache);
subsystems.register_ref::<LiveKeySet>(&mut live_keys);
```

They import only from `pulsar_rendering::subsystems` — never from
`pulsar_rendering::components`.

## LiveKeySet Stale-Cleanup

`LiveKeySet` is a `HashSet<String>` wrapper. At the start of each sync pass
the central system creates a fresh `LiveKeySet`. Every component inserts its
`scene_object_id` into the set. After the sync pass, the central system
removes subsystem entries whose keys are not in the set.

This is used by:

1. **Script stale-cleanup** — `SCRIPT_REGISTRY.lock().retain_keys(live_keys.inner())`
   removes script registrations for deleted objects.

2. **Object-cache stale-cleanup** — `SceneObjectCache` entries whose
   `scene_id` is not in `LiveKeySet` are removed from the cache and their
   helio scene objects are destroyed.

## Mesh Pipeline

```
resolve_asset_path(project_root, mesh_asset)
        │
        ▼
  [check MeshCache by abs_path]
        │
   ┌────┴────┐
   │         │
  HIT       MISS
   │         │
   │    load_mesh_upload(path)
   │         │
   │    scene.insert_actor(MeshActor)
   │         │
   │    scene.insert_material(...)
   │         │
   │    MeshCache.insert(abs_path, (mesh_id, mat_id))
   │         │
   └────┬────┘
        │
        ▼
  ObjectCache check / insert / update
```

Asset resolution order (in `resolve_asset_path`):

1. Absolute path
2. Project-root-relative (`project_root / asset`)
3. Cwd-relative
4. `cwd/assets/` (editor convention)
5. Engine built-in assets

## Object-Instance Cache (`SceneObjectCache`)

Prevents cascade-free of meshes/materials in the helio scene on every frame.

The editor used to **clear all objects each sync pass**, then re-insert them.
When helio's `remove_object` causes a mesh/material ref_count to hit zero,
it cascade-frees the GPU resources. The next insert fails because the handles
are stale.

`SceneObjectCache` tracks `scene_object_id → (helio::ObjectId, mesh_asset)`.
On each frame the component:

1. Looks up its `scene_object_id` in the cache
2. **Hit + same mesh**: calls `scene.update_object_transform()` — no
   destroy/recreate
3. **Hit + different mesh**: removes old object, removes cache entry,
   falls through to insert
4. **Miss**: calls `scene.insert_actor(SceneActor::object(...))`, caches
   the result

Components mark themselves live in `LiveKeySet` so stale cleanup doesn't
delete entries for objects that still exist.

## Component Contract

Every component that wants runtime behavior implements `ComponentRuntimeBehavior`:

```rust
pub trait ComponentRuntimeBehavior {
    const CLASS_NAME: &'static str;
    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    );
}
```

Registered via `inventory::submit!(RuntimeBehaviorRegistration { ... })`.
The central system iterates all component instances and calls
`apply_runtime_behavior_for_class(...)` for each.
