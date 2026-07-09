# Pulsar

> A next-generation game engine and editor built in Rust.
>
> Unlike Unity or Unreal, Pulsar **code-generates a separate project** that
> compiles and runs independently. Your game is not a script running inside the
> engine — it is its own binary with minimal dependencies on the engine's API
> surface. This produces a tiny game with strong compile-time guarantees that
> it is compatible with the engine's file types, formats, and protocols.

## The core insight

Most game engines follow one of two models:

1. **Embedded scripting** (Unity/Unreal) — your game logic runs inside the
   editor process or a runtime VM. You depend on the full engine. Your binary
   is the engine with your script bolted on.
2. **Library + codegen** (Pulsar) — the engine is a toolkit that **generates
   a standalone Rust project**. Your game links only the runtime crates it
   actually uses. The generated project has compile-time guarantees that its
   types, assets, and blueprints are compatible with the engine's formats.

Pulsar is not a runtime you ship — it is a **factory that produces your runtime**.

### How it works

You build your game in the Pulsar editor. When you export, the engine:

1. **Code-generates a Rust crate** containing your game's reflected types,
   blueprint graphs, scene definitions, and asset manifests as Rust code
2. **This crate compiles standalone** against `pulsar_core`, `pulsar_ecs`,
   `pulsar_reflection`, and the renderer — minimal deps, no editor, no GPUI
3. **Compile-time guarantees** — if the generated code compiles, your types
   match the engine's expectations. No runtime format mismatch, no missing
   fields, no serialization surprises
4. **Tiny binaries** — your game doesn't carry the editor shell, the plugin
   system, the AI copilot, or the asset pipeline

The editor itself also runs as a Pulsar project — `Pulsar-Native/crates/core/engine`
is the editor binary. The same codegen path produces it. The plugin system
(`plugin_editor_api`, `plugin_manager`) exists for editor extensions, not for
game logic.

### Comparison

| | Unity/Unreal | Pulsar |
|---|---|---|
| Your game is... | Scripts in the engine process | A standalone Rust binary |
| Binary size | Engine + your script (~100 MB+) | Only what you use (~1-10 MB) |
| Format safety | Runtime deserialization | Compile-time type checking |
| Engine dep | Full engine required | Minimal runtime API surface |
| Editor in game | Ships editor DLLs | Zero editor code |

The same `EngineClass` components, ECS archetypes, and reflection types serve
both the editor (for inspection/editing) and the compiled game (for runtime
performance) — but the game only pays for what it uses.

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
