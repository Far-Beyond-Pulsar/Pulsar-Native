# Script VM refactor plan

Goal: the engine core knows exactly one thing about scripting: how to load,
verify and execute **engine bytecode** against the SceneDB world. Every
language (Blueprints first, TypeScript later) is a plugin that compiles to
that bytecode. Scripts reference the world through typed entity/component
handles, and can call methods registered on reflected types and SceneDB
components.

This plan is based on three read-only investigations of the current code
(bytecode/executor, Blueprint plugin boundaries, reflection/SceneDB methods).
File references are to Pulsar-Native unless noted.

## Where we are

What works:

- An end-to-end path from Blueprint graph to running code exists: the editor
  compiles a graph with PBGC into `bytecode.json`; `pulsar_game`'s
  `BlueprintDispatcher` loads it, binds instances to entities, and runs
  begin_play/tick/end_play inside the TickLoop, with hot reload.
- Component property get/set and method calls work from the VM and from
  generated Rust, through one reflection dispatcher
  (`pulsar_world_registry::dispatch`).
- `ActorRef`/`ComponentRef` (`pulsar_script_object_model`) resolve lazily
  against the live world with typed errors.
- Tests for `pulsar_bp_executor`, `pulsar_std_bundle` and `pbgc` pass.

What is wrong or missing:

1. **The engine bytecode lives inside the Blueprint compiler.**
   `Instruction`/`BpProgram` and the VM are in `crates/third-party/pbgc`
   (`bytecode/`, `vm/`), which depends on `pulsar_std` and `graphy`. The
   runtime (`pulsar_bp_executor`, `pulsar_game`) depends on pbgc to get them.
2. **The VM is not memory safe for non-`Copy` values.** Values are untyped
   bytes in an arena; native shims `ptr::read` arguments, so a `String` or
   `Vec` read twice is freed twice, and heap values left in the arena leak.
   Generic natives dispatch on `size_of` and panic on unknown sizes.
3. **Control flow is wrong in bytecode.** Only branch-shaped nodes and
   `sequence` lower correctly. Loops, switch, gate, do_once, do_n, delay and
   flip_flop run their body once and fire every exec output.
4. **Native calls are resolved by name with dlsym** against a hand-maintained
   whitelist (`ALLOWED_NODE_NAMES`, 180 lines), from a pulsar_std cdylib that
   a build script compiles and embeds. Component ops are encoded as
   `comp_*::Class::Member` strings with JSON operands in 4 KiB arena blobs.
5. **Runtime lifecycle bugs.** The editor names tick/end-play programs
   `on_tick`/`on_end_play`, the dispatcher runs `tick`/`end_play`, so
   editor-compiled tick and end-play graphs never run. `delta_time` is never
   passed. Editor output has `variables: []`, so defaults and level overrides
   never apply. `emit_event` is an empty stub.
6. **Methods cannot be looked up by reference type.** Reflection has method
   registries, but they are keyed by class-name strings, rebuilt on every
   lookup, and callers panic on bad arguments. The authoring macro for
   hand-written methods (`#[component_methods]` + `#[method]`) does not
   compile when used, so the only methods today are generated property
   accessors. SceneDB has no erased access by `(Entity, ComponentId)` and no
   way to list an entity's components.
7. **Blueprint concepts are spread through core.** The graph model is in the
   UI library (`ui::graph`); `pulsar_game` carries a second, dead
   graph-to-bytecode compiler; editor crates depend on the plugin crate
   directly; reflection has `MethodType::{Pure, Fn, ControlFlow}`; SceneDB
   still exports dead `ComponentStore`/`__bp_*` helpers; the plugin API has
   no way to register a language or compiler.

## Target layering

```
pulsar_reflection        types; TypeId-keyed method registry (language-neutral)
pulsar_scenedb           entities, components, erased access, typed handles,
                         ComponentId-keyed component method registry
pulsar_script_vm  (new)  module format, verifier/linker, interpreter, values,
                         native function registry, host (world) interface
pulsar_script_runtime    script instances bound to entities, lifecycle
  (from pulsar_game::blueprint_runtime)  events, hot reload, level bindings
plugin_editor_api        + ScriptLanguage registration (source -> module)
blueprint_editor plugin  graph model, graph compiler -> module, node palette,
                         Blueprint pin semantics, Rust-source export
```

Core crates never mention graphs, nodes, pins, exec flow or Blueprints.

## Key design decisions

**D1. A new typed VM instead of the byte arena.** Registers hold `Value`s
(unit, bool, integers, floats, string, entity handle, component handle,
opaque reflected value). Natives receive `&mut [Value]` and return
`Result<Value, ScriptError>`; no raw pointers, no `extern "C"` shims, no
size-dispatch. Instructions include real jumps, so loops and switches are
ordinary control flow. A step budget and typed errors replace panics. The
existing arena VM is small, but making it memory safe means rewriting every
native shim anyway.

**D2. Natives are registered by name and signature, and can hot-reload.**
A module lists the natives it imports by stable qualified name plus
signature. The linker resolves them against an in-process registry (std
functions, reflected-type methods, component methods) and the verifier
type-checks every call before the module can run. Anything not registered
cannot be called, which replaces the whitelist and raw dlsym. Natives can
come from the engine binary or from a **native library** loaded at runtime:
the library exposes one registration entry point that adds its functions to
the registry. Reloading a library unregisters its natives, loads the new
build, and re-links and re-verifies every module that imports them; a
signature change fails verification instead of calling a stale pointer.
(User decision: native hot reload is a requirement.)

**D3. One method model, two keys.** Reflection gets a TypeId-keyed
registry of `ReflectedMethod { name, receiver (none/ref/mut), params, ret,
flags (side-effect-free, deterministic), attrs, invoke }` using static
fn pointers, with argument validation in the generated shim. SceneDB gets a
ComponentId-keyed registry derived from it, plus explicit methods that
receive `(&mut World, Entity)`. Blueprint's Pure/Fn/ControlFlow become
Blueprint-side interpretations of the neutral flags.

**D4. Handles are the reference types.** SceneDB provides
`ComponentHandle<T>` and an erased `ComponentRef { entity, component }`,
both liveness-checked on every access. Script values carry these at runtime;
anything persisted (graphs, level files) stores StableId plus a stable type
name, because `ComponentId` is process-local.

**D5. Languages are plugins.** `plugin_editor_api` gains a `ScriptLanguage`
extension: file types it owns, a compile function returning a
`pulsar_script_vm::Module`, and a query API over the native registry so a
frontend can list functions and "methods callable on a reference of type X".
The Blueprint plugin moves its graph model out of `ui` and its compiler out
of pbgc; the Rust-source export stays a Blueprint plugin feature.

## Phases

Each phase leaves every touched crate building and tested.

**Phase 0: groundwork (SceneDB, reflection).**
- SceneDB: `World::has_component`, `component_ids(entity)`, `get_dyn`,
  `get_dyn_mut` (a `MutDyn` guard reusing the existing erased hooks);
  `ComponentHandle<T>` and `ComponentRef`; delete `component_store.rs` and
  the `__bp_*` exports.
- Reflection: TypeId-keyed `ReflectedMethod` registry and a working
  `#[reflect_methods]` / `#[reflect_method]` authoring macro; keep the
  existing registries working as adapters.
- SceneDB: ComponentId-keyed component method registry and
  `World::call_component_method`, plus a derive for world-receiving methods.

**Phase 1: `pulsar_script_vm`.**
- Module format (versioned; serde now, compact binary later): functions,
  registers, constants, imports, exported entry points with signatures.
- Value model, interpreter, step budget, typed errors.
- Native registry and linker/verifier; adapters that expose pulsar_std
  functions and reflected/component methods as natives.
- Host interface: the world and the bound entity passed explicitly, no
  thread-local.
- Tests: arithmetic, control flow (loops, switch), strings without
  double-frees, handle liveness, method calls on components, verifier
  rejections.

**Phase 2: runtime on the new VM.**
- Move `pulsar_game::blueprint_runtime` to `pulsar_script_runtime` with
  neutral names; run modules instead of `BpProgram`s.
- Fix the lifecycle: canonical entry points (`begin_play`,
  `tick(delta_time)`, `end_play`, custom events), variable defaults and
  level overrides, hot reload.
- Neutral level-file bindings and project-builder discovery.

**Phase 3: plugin API.**
- `ScriptLanguage` registration and native-registry query API
  (`functions()`, `methods_for(type)`).
- Generic pre-play validation hook and menu contributions, so editor crates
  stop depending on the Blueprint plugin crate.

**Phase 4: Blueprint plugin.**
- Move the graph model out of `ui::graph`; move the graph compiler out of
  pbgc; compile graphs to `pulsar_script_vm::Module`.
- Palette from the registry, including methods by reference type and the
  global node list.
- Keep the Rust-source export inside the plugin.

**Phase 5: removal.**
- Delete pbgc's bytecode/VM, `pulsar_bp_executor`, `pulsar_std_bundle`,
  `pulsar_game`'s dead bytecode compiler, the dylib shims in
  `pulsar_macros`, and the Blueprint names left in core.

## Decisions taken

- D1: typed register VM (user confirmed).
- D2: native libraries stay hot-reloadable, registered by name + signature
  (user confirmed).

## Open question

Phase 4 needs push access to the Blueprint plugin repository
(`plugins/vendor/blueprint_editor`); `crates/third-party/pbgc` and `graphy`
are also separate repositories.
