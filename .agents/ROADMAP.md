# Pulsar

> A next-generation game engine and editor built in Rust.
>
> Pulsar is not a "game engine" in the Unity/Unreal sense — it is a **platform for
> building interactive worlds** where the editor IS the runtime, the runtime IS the
> editor, and the boundary between "tool" and "game" is a compile-time choice.

## The core insight

Most game engines keep the editor and the runtime in separate processes (or at
least separate binary modes). The editor is a tool that produces data; the
runtime consumes it. This split creates a fundamental impedance mismatch:
editor systems cannot access runtime data directly, runtime systems cannot
leverage editor tooling, and every change requires a serialization round-trip.

Pulsar collapses this. The engine is a library that can be linked in two modes:

1. **Editor mode** — everything loaded: the GPUI-based editor shell, reflection
   system, blueprint graph runtime, AI copilots, plugin system, asset pipeline.
2. **Runtime mode** — headless: the ECS, physics, rendering, scene graph, and
   blueprint executor with no GPUI dependency.

The same `EngineClass` components, the same ECS archetypes, and the same
subsystems serve both modes. An `#[engine_class]` defined in a plugin DLL
is editable in the editor's property panel AND ticked by the runtime's ECS
schedule — no conversion step, no serialization boundary.

## Architecture philosophy

### Everything is a component

The building block of a Pulsar world is the **engine component** (`EngineClass`).
Components carry reflected properties (serializable, inspectable in the editor),
blueprint-callable methods, and per-tick runtime behavior via
`ComponentRuntimeBehavior`. They are the atoms of the object model.

Components are not tied to the ECS — they exist as a higher-level abstraction
that the ECS (`pulsar_ecs`) can host at runtime via `ComponentStore`. A
component can be attached to an entity in a `.scene` prefab, edited in the
level editor's property panel, serialized to JSON, and ticked in the runtime.

### The editor is a GPUI application

The entire Pulsar editor UI is built on **WGPUI** (`crates/ui/wgpui`), a
community fork of Zed's GPU-accelerated UI framework. The editor is not a
separate program — it is a GPUI `App` that loads editor-specific subsystems.

### Plugins are native Rust DLLs, never unloaded

Pulsar's plugin system is the opposite of a sandboxed scripting runtime.
Plugins are `cdylib` Rust libraries loaded via `libloading` and **never
unloaded** (the `Library` handle is wrapped in `ManuallyDrop`). This eliminates
the entire class of FFI safety problems (dangling vtables, stale function
pointers, `Arc` drop across a boundary). The cost is that plugins occupy
virtual memory for the process lifetime — negligible for 2-10 MB editor DLLs.

Plugins share the exact same `gpui-ce`, `ui`, `serde`, and `plugin_editor_api`
crates with the engine — same compiler version, same ABI. The `export_plugin!`
macro generates the FFI boundary, and `VersionInfo` enforces that the rustc
version matches.

### The filesystem is virtual

No code in Pulsar calls `std::fs` directly. Everything goes through
`engine_fs::virtual_fs`, which routes to a pluggable `FsProvider` — local disk,
remote HTTP (`pulsar-studio`), or P2P (`pulsar-multiplayer-core`). This means
the same editor can open a local project, a cloud-hosted project on a remote
server, or a peer's workspace, with no code changes.

### State is typed and reactive

Instead of `OnceLock<RwLock<T>>` globals or a god-object `EngineContext` with
named fields, Pulsar uses `StateStore` — a `TypeId`-keyed resource table.
Any type `T: Default` can be inserted and retrieved without upfront
registration. `ResourceHandle<T>` provides lock-free reads, RAII writes with
automatic version bumping, and async change notifications via `.changed()`.

### The type system is two-tier

1. **Compile-time types** — Rust structs/enums with `#[derive(Reflectable)]`,
   auto-registered via `inventory`. Used for the majority of reflected types.
2. **Dynamic types** — types composed at runtime via `DynamicTypeBuilder`,
   stored in `DYNAMIC_TYPE_REGISTRY`. Used for user-defined `.alias.json`
   types and plugin-provided component schemas.

This mirrors the engine's overall philosophy: use compile-time guarantees
where possible, but provide escape hatches for runtime extensibility.

## Project position

Pulsar aims to eventually challenge Unity and Unreal as a first-class game
development platform. This is a long-term ambition, not a short-term claim.
The path is through technical differentiation, not feature parity.

Near-term, Pulsar targets:

- **Technical artists and small studios** who want a Rust-native toolchain
- **Tool developers** building domain-specific editors (e.g., a VFX graph,
  a dialogue tree, a custom simulation)
- **Education and research** into real-time engine architecture
- **The "editor is the game" model** — standalone applications that include
  their own editing capabilities

Long-term, the thesis is that native Rust ergonomics, a reflection-first
architecture, and deep AI integration represent a genuine step forward in
engine design — not just a side project for enthusiasts.

Pulsar prioritizes:

1. **Native Rust ergonomics** — no scripting language, no binding layer,
   full native performance for both editor and runtime
2. **Reflection as infrastructure** — the entire editor is built on runtime
   type information, not hardcoded property editors. New types get editor
   support for free
3. **Plugin-first design** — every built-in editor is also a plugin; the
   plugin API is not an afterthought. This forces API quality
4. **AI copilot integration** — every plugin can expose AI-accessible tools
   through the agent provider system. The engine is being built for the
   AI-assisted workflow from day one
5. **Unified editor/runtime** — the same `EngineClass` components, ECS
   archetypes, and subsystems serve both editor and published game. No
   serialization boundary between editing and playing

## Repository structure

Pulsar-Native is the monorepo containing the engine, editor, and all vendored
dependencies as git submodules with path deps. See `crates/` for the crate
tree, `plugins/vendor/` for editor plugin submodules, and `.agents/` for
detailed documentation on each subsystem.
