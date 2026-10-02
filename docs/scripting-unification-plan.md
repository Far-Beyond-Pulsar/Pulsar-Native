# Scripting unification plan

Prepared 2026-10-02 against `4aced8e9d05ccad244d216f8b090957c49bd30a4` and the thirteen linked issues. This is an implementation plan; no runtime changes have been made or tested.

## Decision

Use one language-neutral, verified script module as the execution contract. Blueprint and TypeScript compile into that contract. The interpreter and standalone Rust export consume it. SceneDB owns live component access; reflection describes callable behavior; plugins own language syntax and editor presentation.

Preserve standalone Rust export. Replace its independent graph-to-Rust implementation with a module-to-Rust backend, rather than maintaining two definitions of Blueprint behavior. Use the existing Module as the initial common representation; introduce another IR only if a measured requirement cannot be met by it.

“Eliminate non-ideal patterns” here means removing duplicate models, competing execution semantics, legacy dispatch, and language-specific dependencies in the affected scripting paths. It does not mean an unrelated repository-wide rewrite.

## What the current code actually does

| Evidence | Consequence for the plan |
|---|---|
| `plugins/vendor/blueprint_editor/compiler/Cargo.toml` depends on Graphy and the VM, without GPUI. Its `src/lib.rs` accepts `graphy::GraphDescription`. | #883 is partly solved. Consolidate the editor's duplicated model and conversions; do not introduce a third graph model. |
| `crates/ui/wgpui-component/crates/ui/src/graph/mod.rs` defines graph assets, variables, nodes and pins; the plugin converts these to Graphy. | Extract the authored asset schema and share it with headless tooling. |
| `pulsar_script_vm/src/adapters.rs` already uses SceneDB `get_dyn`/`get_dyn_mut`, reflected methods, and `MethodFlags`. | Extend this route rather than adding another component dispatcher. |
| `pulsar_world_registry/src/dispatch.rs::property_metadata` creates a default EngineClass on every lookup. `script_natives.rs` already captures property metadata at registration time. | Benchmark and fix the actual paths separately. The per-call construction claim does not apply equally to both. |
| `pulsar_script_runtime/src/lib.rs::reload_class` preserves variables by name and exact type and rebases compatible waiting calls. | Add identity-based migration to the existing transactional contract and continuation reports. |
| `pulsar_script_vm/src/module.rs` has scalar-only constants, binary encoding, source locations and format version 2. | Extend the codec and verifier together; reuse existing debug information. |
| `ui_core` directly links and wraps `blueprint_editor_plugin`. | Move all of that wrapper's capabilities behind plugin registration, including AI tools and scripting. |
| PBGC still exports bytecode/VM APIs. `pulsar_std` flow functions still contain global state and blocking sleeps. | Remove the obsolete stack after replacing consumers and validating export parity. |

The `.agents` docs contain historical crate locations and architecture details. Update the relevant docs at completion; current manifests and implementations take precedence during this work.

## Required invariants

- Core reflection and execution know types, effects, attributes, imports, state and source locations, without interpreting Blueprint nodes.
- One authored graph schema, one native signature catalog, and one component method implementation serve every relevant consumer.
- Each script instance owns its state. Stateful node identity survives renames, graph layout changes and macro expansion.
- Waiting uses game time and continuations. No flow node blocks the game thread.
- Component writes hold SceneDB mutation guards until after mutation. Cached descriptors never retain entity pointers or borrowed components.
- Side-effect-free does not imply deterministic: reading a world transform must not become a compile-time constant.
- Migration and validation errors are visible and actionable. Unsupported constructs never silently become defaults or no-ops.
- Use `engine_fs::virtual_fs` for project I/O in the affected compilation/migration paths, with byte-oriented compiler APIs usable by standalone hosts.

## Delivery order

### 1. Establish executable contracts and an inventory

Capture representative existing graphs, modules, class overrides and expected behavior before changing schemas. Inventory PBGC consumers, native signatures, component properties, duplicate registrations, graph conversion callers and plugin wrapper capabilities.

Create a headless conformance harness with a fake game clock and deterministic host. Assert state, native-call traces, errors and scheduling, not only printed logs. Run generated Rust in both debug and release modes. Record the pre-change property benchmarks here.

**Gate:** every subsequent phase has a fixture proving old data can be read or an explicit diagnostic explaining why it cannot.

### 2. Consolidate the graph model — #883

Create a GPUI-free `blueprint_graph` crate in the Blueprint plugin repository for the authored asset envelope, class variables, events, macro library and graph metadata. Reuse Graphy's node/pin/connection types where they express the persisted schema. If their schemas differ, define an explicit versioned conversion at the import boundary; remove parallel in-memory definitions after migration.

Keep interaction state, GPUI entities, theme colors and rendering adapters in the UI/plugin. Persist layout using plain numeric data. Make graph loading, macro expansion and migration callable headlessly. Remove `convert_ui_graph_description_to_pbgc` and equivalent conversions once all consumers use the canonical model.

Assign stable variable IDs when variables are authored; persist IDs rather than generating them on every compile. Derive hidden state IDs from class, macro-instance expansion path, node ID and state-field identity, so two macro instances cannot share state.

**Gate:** old graph fixtures round-trip without losing connections, defaults, IDs or layout; compiler dependency inspection contains no GPUI or editor shell.

### 3. Finish language-neutral reflection — #884, #885

Use the existing `MethodFlags { side_effect_free, deterministic }` and free-form attributes as the common method metadata. Keep Blueprint interpretation of execution pins and selector outputs in the plugin. Store their metadata as attributes without teaching the reflection core their meaning.

Migrate legacy `MethodMetadata`, dynamic method registration, native adapters, proc-macro output and all consumers before deleting `MethodType`. Parse macro arguments structurally with `syn`; remove string/token-substring detection. Preserve the distinction between pure computation and a read of changing world state.

Delete `engine_class_derive::component_methods` and its obsolete method-registration machinery after an exact call-site and documentation audit. Retain SceneDB's `component_methods` with `reflect_method`/`world_method` as the canonical component API. Remove only obsolete generated hooks, preserving unrelated EngineClass features.

**Gate:** macro compile-pass/fail tests, duplicate-method diagnostics and repository searches show no active legacy enum or macro consumers.

### 4. Add script value types and literals — #870, #871

Register Vec2, Vec3, Vec4, Quat and Mat4 using existing value-type registration. Audit actual transform precision and glam versions before deciding DVec3 coverage. Put concrete registrations in a runtime integration crate linked by editor and packaged games, without introducing a renderer dependency into the VM.

Provide constructor, split/field, equality and display natives using registered type metadata. For foreign glam types, use supported external registration or free-function natives rather than an illegal inherent impl. Keep raw equality distinct from approximate vector comparison and quaternion rotation equivalence. Specify matrix element order and numeric conversion rules in the serialized format.

Extend Constant with a named typed payload and a registered, fallible decoder. Resolve each literal once at link time and clone it with value semantics at use/instantiation; mutating one instance must never mutate the pool or another instance. Reuse reflection serialization hooks where available. Reject unknown types and malformed payloads with source diagnostics.

Choose a payload representation with an explicit bincode encoding: the proposed `serde_json::Value` cannot simply be added to the current derived Encode/Decode schema. Bump FORMAT_VERSION, retain documented legacy JSON decoding, and rebuild binary artifacts under the current-version-only binary policy. Update verifier, linker, defaults and compiler lowering together. Pin colors belong to Blueprint presentation metadata.

**Gate:** all math types pass construction, field access, native argument/return, default-literal and JSON/binary round-trip tests; clone isolation and malformed values are covered.

### 5. Make state migration identity-based — #867

Add stable class identity, class schema version and variable identity independently of bytecode format version. Carry them from source assets through compiler inputs, Module and state/override records. Identify legacy variables by name only when stable identity is absent; reject ambiguous or conflicting IDs rather than applying an arbitrary match.

Use a shared migration service for hot reload, level overrides and saved variable state. Match IDs first, preserve compatible types, initialize additions from defaults and report removed/retyped values. Keep unmapped authored data recoverable instead of silently discarding it. The migration envelope records old/new schema versions and type identities; it must not serialize raw entity handles as persistent identity.

Support explicit, ordered migration hooks for type changes. Specify a bounded state-in/state-out contract with no world mutation or suspension. Validate and migrate every affected instance into staging state before committing code, event declarations/subscriptions and state together. Any failure leaves the old class running. This also requires auditing the current pre-link `declare_events` call for rollback behavior.

Extend ReloadReport with kept, migrated, defaulted and incompatible variables and route it to editor problems. Preserve the existing continuation-rebase checks; do not map old instruction positions solely from variable IDs. Report cancelled waits separately.

TypeScript fields need persistent identity too: use explicit IDs or a persisted class-schema sidecar, with an explicit rename operation when inference is ambiguous.

**Gate:** rename, reorder, add, remove, retype, multi-version migration, malformed IDs, hook failure and waiting-call tests pass. Hot reload and override migration are delivered here; integrate with any existing save/load service, or provide and test the reusable state codec without claiming a complete game-save system.

### 6. Move component behavior onto SceneDB — #886, #887

Start with the registered transform-facing component types and trace their relationship to scene-model Transform and renderer components. Do not assume every type named Transform is the same runtime component. Then cover lights, mesh, physics and all remaining world registrations from the inventory.

Declare methods once with SceneDB component_methods. VM natives, Blueprint palette entries and TypeScript declarations derive from the same method catalog. Use world_method when validation needs neighboring components or world state. Preserve entity liveness, component presence and prefab slot identity checks.

Cache immutable property descriptors by component type/schema generation for editor property access. Replace throwaway EngineClass construction and raw mutable bridges with scoped SceneDB access. Let guard completion drive notifications and GPU synchronization; verify actual observer behavior before removing explicit refresh calls.

Enforce Movability in component mutation methods using the existing authored-to-Helio policy. Runtime moves of Static objects return a typed failure; authoring changes follow the existing warning/confirmation policy. Prevent auto-generated raw field setters from bypassing these methods. Define explicit-method precedence or suppress generated setters for protected properties; duplicate registration must fail visibly.

Migrate old `comp_get_prop`, `comp_set_prop` and `comp_call` graph nodes to canonical native methods with explicit pin mappings. Preserve receiver wiring, slot IDs, defaults and execution edges. Retain old spelling only in the versioned importer, then remove per-property native generation from world_registry once coverage is complete. Cached modules with old imports must rebuild or fail with migration guidance.

**Gate:** transform writes update physics/render subscriptions correctly; stale handles and Static writes fail correctly; legacy graphs migrate idempotently. Benchmark direct typed access, current generic dispatch, replacement dispatch and VM native access with allocation counts and release-mode latency. Require zero temporary EngineClass construction per access; do not invent an overall speedup target before measuring.

### 7. Unify Rust export and delete obsolete execution — #874, #876, #875

Implement a GPUI-free module-to-Rust backend, provisionally `pulsar_script_codegen`, over the verified Module. Blueprint lowers stateful flow exactly once into variables, jumps and Wait. Generated Rust stores those variables per instance and emits resumable call frames/state machines for waits, including nested calls.

Share arithmetic policy, error taxonomy, native import resolution, capability checks, instruction-budget accounting and call-depth limits with the interpreter. Keep any shared execution support minimal and GPUI-free. Generated output must actually execute generated Rust; embedding the interpreter is not completion of DirectRust parity.

The conformance matrix includes multiple instances and repeated/macro-expanded nodes; gate/reset/counter behavior; delay and retriggerable delay; overflow, division/remainder edge cases and checked arithmetic; component reads/writes; native failures; recursion/budget exhaustion; value types; events and subscriptions. Compare wake times and observable effects under the same fake clock.

After parity, switch export to this backend and remove stateful/global/blocking flow bodies from executable std paths. Keep intrinsic declarations/metadata where needed. Audit multi_gate and retriggerable_delay too, not just the four named functions. Unsupported nodes must fail compilation during the transition.

Move still-used metadata/project-export utilities out of PBGC into their proper compiler/export owners, then delete its bytecode and VM modules, exports, fixtures and dependencies. Audit external consumers through repository searches and known downstream builds; public search cannot prove that no private consumer exists. Publish migration guidance and a versioned breaking release where necessary. PBGC's independent DirectRust backend is retired after replacement; it does not remain a second implementation.

**Gate:** VM/generated-Rust conformance passes in debug and release; standalone exported games link no UI; repository callers no longer depend on PBGC bytecode/VM or old graph-to-Rust execution.

### 8. Make language plugins optional — #882

Remove the Blueprint dependency and wrapper from ui_core. Load Blueprint through the existing permanent-library/plugin API. Transfer file handling, panel creation, language registration, capabilities, AI tools and session operations through generic plugin interfaces; do not move a hardcoded Blueprint import into another shell module.

Keep compiler libraries independently usable by headless packaging. If ScriptLanguage/diagnostic contracts need extraction from the GPUI-dependent SDK, put them in a small language-neutral crate and re-export from plugin_editor_api. Editor discovery and headless compilation must select the same language IDs and compiler implementations.

**Gate:** editor builds and starts without Blueprint installed; Blueprint installs through normal discovery and retains its capabilities; an explicitly required missing language blocks compilation with a useful diagnostic. CI uses an explicit editor package and dependency check because workspace-wide builds may intentionally include plugin packages.

### 9. Add TypeScript as the second frontend — #880

Create a separate plugin plus GPUI-free compiler. Evaluate swc/oxc against the required subset, license, MSRV and diagnostics, then pin one parser; a parser alone is not a type checker. Implement subset checking against an immutable NativeRegistry/TypeRegistry catalog, then lower to the same Module and verifier.

Define the language contract before implementation: typed class fields/methods, control flow, native calls, math values, entity/component references and `await wait(seconds)`. Define numeric mapping explicitly: TypeScript number maps to VM float; expose an explicit engine integer type/conversion rather than silently treating all numbers as i64. Reject unsupported dynamic JS behavior, closures/generics where unimplemented, arbitrary Promise scheduling and imports outside the supported module model.

Generate deterministic `.d.ts` declarations from the same registry snapshot used to compile/link. Include signature changes in cache invalidation and validate emitted declarations with TypeScript tooling. Emit existing SourceLoc file/line/column ranges. Compile `.ts` classes under `src/classes/<Class>/` through ScriptLanguage, without a JS runtime. Define a language discriminator so two frontends cannot overwrite the same class artifact.

**Gate:** Blueprint and TypeScript versions of a sample class produce matching observable behavior, including Vec3 updates, waits, errors and migrated field state. Editor autocomplete, headless compilation, packaging and missing-plugin handling work end to end.

## Dependency and integration strategy

Sequence: baseline → graph/reflection → value types and identity migration → component migration → export parity → legacy removal. Plugin decoupling can follow the shared-model boundary; TypeScript integration follows the stable registry/module/plugin contracts. Build its parser prototype earlier only against frozen test fixtures.

Land reviewable changes across Graphy, WGPUI-Component, Blueprint, Reflection, SceneDB if needed, PBGC and the engine. Publish dependency changes before updating parent pins. Check the actual workspace patch graph for duplicate Reflection/SceneDB versions: split Rust type identity would break inventory and native bindings. Do not leave permanent compatibility layers; each temporary adapter has a fixture, removal dependency and final deletion gate.

## Final acceptance

| Issue | Closure evidence |
|---|---|
| [867](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/867) | Stable IDs/versioned schemas, transactional state migration, visible loss reports. |
| [870](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/870) | Registered math values, required natives and Blueprint pin presentation. |
| [871](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/871) | Typed defaults compiled and linked once, with codec and isolation tests. |
| [874](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/874) | Per-instance flow state and nonblocking generated waits. |
| [876](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/876) | Shared module semantics and passing VM/Rust conformance. |
| [875](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/875) | Old PBGC VM removed, downstream audit and export replacement completed. |
| [880](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/880) | Working TypeScript subset plugin, declarations, diagnostics and packaging. |
| [882](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/882) | Blueprint-free editor CI plus normal plugin installation. |
| [883](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/883) | Shared GPUI-free graph schema and deleted duplicate conversions. |
| [884](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/884) | MethodType removed; neutral flags/attributes used end to end. |
| [885](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/885) | Obsolete macro and references removed. |
| [886](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/886) | Scoped live access, no temporary metadata instances, benchmark comparison. |
| [887](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/887) | Built-in method coverage, node migration and enforced Movability. |

Run focused crate tests at each phase, separate plugin/submodule tests where outside the workspace, dependency checks, then the repository's `just check`, `just test`, `just clippy` and `just build` for final integration. Record existing failures separately. Update scripting, plugin and architecture docs to describe the resulting system. No issue is closed solely because its replacement API exists: consumers, old data and deletion gates must also pass.

## Status (2026-10-02)

| Phase | State |
|---|---|
| 1. Baseline and contracts | Done: `docs/scripting-baseline.md`. The persisted-graph fixture gap is closed (`blueprint_graph/tests/fixtures/graph_save.json`), the `pulsar_game` scripting tests pass, and the shared-variable probe was replaced by an acceptance test that exported actors keep per-actor variables in debug and release (`just ci-drift-check`, ignored). |
| 2. Graph model (#883) | Done. `blueprint_graph` (GPUI-free) holds the authored schema; `ui::graph` is gone from wgpui-component; `convert_ui_graph_description_to_pbgc` is removed and the headless compiler depends on the model without UI. Hidden node state has stable ids derived from the node id (macro expansion paths are part of node ids), and variables have stable ids. |
| 3. Reflection (#884, #885) | Done. `MethodType` is removed: `MethodMetadata` and `DynMethodMetadata` carry `MethodFlags`. Derive-generated getters are `side_effect_free` but not `deterministic`; setters are `NONE`. The registry snapshot emits `flags`. `engine_class_derive::component_methods` and its `#[method]` parser are deleted (no users). `ComponentMethodRegistration` stays because the derive still uses it for auto property accessors. |
| 4. Value types and literals (#870, #871) | Done. `pulsar_script_math` registers Vec2/Vec3/Vec4/DVec3/Quat/Mat4 with constructor, field, arithmetic, raw and approximate equality, and to-string natives (linked through `pulsar_world_registry`). `Constant::Value { ty, json }` (format version 3) is decoded once at link time and cloned per use; the Blueprint compiler emits it for unconnected pins and validates it per node. Math pin colours live in the plugin. Matrices are column-major. |
| 5. State migration (#867) | Done. Variables and modules carry stable ids and a class version; `pulsar_script_vm::migrate` matches by id (name only when an id is missing, never across different ids). `reload_class` is transactional (stage, `migrate` hook, then commit) and reports kept/renamed/defaulted/incompatible/removed variables; `SavedState` saves and restores through the same migration. Blueprint passes variable ids; authoring a class version and `migrate` hook in the editor is not built. Event declarations cannot be retracted by the hub, so a failed reload can leave new events registered. |
| 6. Component behaviour on SceneDB (#886, #887) | Partly done. #886 done: property descriptors are cached per class (`property_descriptor`, shared with `pulsar_script_object_model`), with before/after numbers in the baseline doc. #887: `Transform` now has a `#[component_methods]` script surface (`position`, `rotation_degrees`, `scale`, `set_*`, `translate`) and is a script component; setters go through neutral `MotionGate`s that Helio registers for `Movability`, so a Static object cannot be moved at runtime. Not done: the other built-in components (lights, mesh, physics, ...) still use the generated per-property natives, so those natives and the `comp_get_prop`/`comp_set_prop` node migration remain. Transform was never script-visible before, so it has no legacy nodes. Helio change is committed locally on a branch in the Helio repo and not pushed. |
| 7. Rust export, parity, PBGC (#874, #876, #875) | Done. `pulsar_script_codegen` generates one compiled `step` per module function; generated code runs on the same VM (shared linking, instances, calls, waits, limits, errors, `exec` operators). `pulsar_script_conformance` runs hand-written and Blueprint-compiled modules interpreted and generated, comparing results, errors with traces and locations, variables, native calls and waiting, in debug and release (`just conformance`). The exported `Actor` keeps script state per actor (`pulsar_game::scripting::export`); waits use wall-clock by default because the pinned `Actor::tick` has no time. Classes with custom events are refused. Stateful `std` flow nodes are `intrinsic: true` declarations (no globals, no sleep, no native). PBGC is reduced to the node-metadata bridge (0.2.0, breaking). Remaining differences: exported actors measure waits in wall-clock, not game time, and cannot subscribe to events. |
| 8. Optional language plugins (#882) | Done. `ui_core` no longer depends on the Blueprint plugin or wraps it. The plugin implements `BuiltinEditorProvider` itself (feature `builtin`) and registers at link time through `plugin_manager::LinkedEditorProvider`; `pulsar_engine` has a default `blueprint` feature that links it. `cargo check -p pulsar_engine --no-default-features` (a CI step and `just check-no-blueprint`) builds the editor without it, and the CI dependency check now fails if `ui_core` depends on a scripting language. Not done: loading Blueprint as a dynamic library instead of a linked built-in (the same registration path serves both). |
| 9. TypeScript (#880) | Done. `pulsar_script_ts` parses with oxc (pinned 0.152, MIT), type-checks against the native registry and lowers to the same `Module` Blueprints produce, with line/column debug info, `.d.ts` generation (verified with `tsc --strict`: `just check-typescript-declarations`), `await wait(s)` as `Wait`, `x as int|number|string` conversions, and field identity in `class.schema.json` (`@renamedFrom`, class version, `migrate` hooks). `plugin_typescript` is the `ScriptLanguage` plugin (links at build time; the packager finds it through `plugin_editor_api::linked_script_languages`, the editor through the provider, and a test asserts both see the same ids). One language per class is enforced (`class.ts` vs `graph_save.json`, `events/.build/language`). A Blueprint and a TypeScript version of one class are proven to behave identically (`blueprint_parity`); migration across TypeScript versions is tested with the runtime. See `docs/typescript-scripting.md`. Not done: custom events, closures, arrays/objects, a TypeScript editor panel. |

Repository notes: the plugin's `engine_fs` git dependency must stay covered by the root `[patch]` section, otherwise Cargo builds a second engine copy and fails in `plugin_manager`. The wgpui-component commits and the plugin changes are local only and need pushing, and the root repo needs its submodule pins updated, before others can build this.
