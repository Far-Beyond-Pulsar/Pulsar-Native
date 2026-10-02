# Scripting unification baseline

This is the phase 1 inventory for [the scripting unification plan](scripting-unification-plan.md).
It records the current executable contracts and the places later phases must
change. It does not claim that the separate generated-Rust and VM backends are
equivalent.

## Reusable headless contracts

The repository already has headless execution fixtures; later phases should
extend these rather than build another test framework.

| Contract | Existing fixture and expected behavior |
|---|---|
| Graph-to-Module compilation | `plugins/vendor/blueprint_editor/compiler/tests/compile.rs` builds Graphy graphs in memory and runs the produced Module in a fresh `World`. `Run` owns a deterministic native registry, entity and VM. Pure arithmetic updates class state; invalid graph inputs return compiler diagnostics. |
| Native trace and instance lifecycle | `crates/core/pulsar_game/src/scripting/tests.rs` installs `test::event`, which records `(event, stable entity id)`. Spawn/destroy tests assert the exact call order, instance state and parity of standalone and PIE startup. |
| Stateful flow and game time | `plugins/vendor/blueprint_editor/compiler/tests/compile.rs::delays_suspend_until_game_time_passes` and `::retriggerable_delays_restart_their_countdown` use `Host::at_time` and explicit timestamps. Delay state is per instance, a second ordinary delay trigger is ignored, and a retriggered delay extends the current wait. VM-level nested continuation coverage is in `crates/core/pulsar_script_vm/tests/interpreter.rs::waits_suspend_and_resume_with_all_state`. |
| Component access and failures | `plugins/vendor/blueprint_editor/compiler/tests/compile.rs` covers `comp_get_prop`, `comp_set_prop`, `comp_call`, explicit component references and slot handles. `crates/core/pulsar_world_registry/tests/component_dispatch.rs` asserts live typed writes, JSON/boxed marshalling errors, missing components and stale entity failures. |
| Runtime errors and diagnostics | `crates/core/pulsar_script_vm/tests/interpreter.rs` checks division by zero, native failure, budget exhaustion and trace frames. Blueprint compile tests assert source/node diagnostics; `crates/core/pulsar_game/src/scripting/tests.rs::script_errors_report_class_function_and_node` checks that runtime/link errors surface as script problems. |
| Class overrides | `crates/core/pulsar_class/tests/instances.rs::overrides_survive_a_class_edit_and_the_rest_updates` captures component slot and variable overrides across a class-default edit. `crates/editor/ui_level_editor/src/core/scene_edit/tests/classes.rs::save_writes_only_overrides_and_load_follows_class_edits` covers the editor save/load route. Variable override identity is currently the variable name. |
| Module and binary compatibility | `crates/core/pulsar_script_vm/tests/binary_format.rs`, `verify_and_link.rs`, and `interpreter.rs` cover current module JSON/binary formats, verification and execution. The module format is version 2; see `pulsar_script_vm/src/module.rs`. |

These form the initial conformance matrix. Generated Rust currently has
compile-time drift checks in `crates/core/pulsar_game/src/blueprint_codegen_drift.rs`
and an ignored full-project compile check in
`crates/core/pulsar_game/tests/generated_project_compiles.rs`. Those checks
validate emitted signatures and compilation; they do not execute the generated
logic against the VM fixtures or compare behavior in debug and release.

One source-level divergence is already visible: VM variables live in each
`pulsar_script_vm::Instance`, while PBGC's project wrapper emits a thread-local
`Cell<Option<T>>` or `RefCell<Option<T>>` for every class variable in
`crates/third-party/pbgc/src/project.rs`. That state is shared by generated
instances of the same class on the same thread. This is a code inspection
finding; the ignored executable probe added to
`crates/core/pulsar_game/tests/generated_project_compiles.rs` records the same
behavior in debug and release when explicitly run. Until that costly probe has
run, there is no runtime profile comparison result.

## Current architecture inventory

### Graph and compiler paths

- The authored UI graph types are in the vendored `ui::graph` crate, consumed
  by `plugins/vendor/blueprint_editor/src/core/graph.rs` and the persisted
  `BlueprintAsset` in `plugins/vendor/blueprint_editor/src/io/formats.rs`.
- Graphy owns a second `GraphDescription` model at
  `crates/third-party/graphy/src/core/graph.rs`. The Blueprint plugin converts
  authored UI graphs in
  `plugins/vendor/blueprint_editor/src/features/compilation/compiler.rs::convert_ui_graph_description_to_pbgc`.
  The conversion is called for local/library macros (two call sites in that
  compiler module) and class validation (two call sites in
  `src/features/validation.rs`). The converter also remaps `macro_entry` and
  `macro_exit`, narrows UI positions to Graphy's numeric fields, and drops
  invalid pin connections.
- The new GPUI-free `blueprint_compiler` in
  `plugins/vendor/blueprint_editor/compiler` accepts Graphy's graph directly
  and emits `pulsar_script_vm::Module`. The Blueprint editor plugin depends on
  it and passes macro-expanded graphs; it is an existing headless frontend
  seam.
- PBGC's graph-to-Rust, graph-to-bytecode and project wrappers remain in
  `crates/third-party/pbgc`. Production code still references PBGC from
  `pulsar_game` (generated project probes and runtime component/codegen
  probes), `code_editor` (manifest dependency), and the Blueprint plugin
  (pinned external dependency for graph metadata, validation and Rust export).
  `pbgc::vm` and `pbgc::bytecode` execution references found by repository
  search are in PBGC's own tests. The exported API and dependency remain
  public and must be audited before removal.

### Native signatures and component properties

- `pulsar_script_vm::NativeRegistry::with_engine_natives` collects native
  providers and rejects a repeated qualified name with `DuplicateNative` in
  `src/native.rs::register`.
- `pulsar_world_registry/src/script_natives.rs::world_component_natives`
  enumerates each registered world component. For every script-visible
  reflected property `p` of class `C`, it registers
  `C::get_p(C&) -> T` and `C::set_p(C&, T) -> ()`. It then registers
  reflected methods as `C::method(C&, args...) -> return` (or `()`). Property
  metadata for these VM natives is captured while the registry is built.
- Generic editor/property dispatch is separate:
  `pulsar_world_registry/src/dispatch.rs::property_metadata` calls
  `REGISTRY.create_instance(class_name)` for each property read or write, then
  scans that temporary instance's properties. This includes the boxed and
  JSON APIs. A normal `EngineClass` property lookup therefore constructs a
  default metadata instance per access in this dispatcher.
- `pulsar_world_registry/src/audit.rs` exposes registry-wide duplicate
  method-name auditing. Reflection tests and physics' checked-in metadata
  snapshot cover reflected surfaces. Duplicate VM native names fail
  registration, while duplicate reflected methods are detectable through the
  audit. Neither should be treated as a single global duplicate-registration
  policy yet.
- `pulsar_world_registry/src/script_natives.rs` still reads legacy
  `MethodType` to infer purity; `MethodFlags` are already attached to native
  signatures. The existing inventory therefore includes two metadata routes.

- Reflected component properties/methods have a snapshot mechanism in
  `pulsar_world_registry/src/audit.rs`; the checked-in
  `crates/subsystems/pulsar_physics/tests/expected_registry_snapshot.json`
  covers the PhysicsComponent metadata linked by that suite. There is no
  checked-in snapshot of every world component and every engine native in one
  executable host, so later inventory changes should add one.

### Existing-data fixture gap

The non-empty Graphy graphs used for compile and behavior contracts are built
in memory in the compiler tests. `plugins/vendor/blueprint_editor/tests/io_formats.rs`
checks current defaults and legacy field conversions, but the repository has
no checked-in non-empty `graph_save.json` round-trip fixture. A representative
persisted authored graph must be captured before changing the saved schema.
Until one is added, this remains an explicit phase 1 gap in the old-data gate.

The reproducible property timing baseline is the ignored
`baseline_property_reads` test in
`crates/core/pulsar_world_registry/tests/component_dispatch.rs`. It measures
direct typed SceneDB reads, boxed reflected reads and JSON reflected reads in
release mode, including allocation/reallocation calls per access. It has no
latency threshold because shared-machine timings are not a stable acceptance
target. A VM-native access path is not included in this pass: the ignored test
is scoped to property dispatch and does not yet own a reusable module/host
fixture.

### Blueprint plugin wrapper

`crates/editor/ui_core/src/builtin_editors.rs::BlueprintEditorBuiltinProvider`
is a direct wrapper around `blueprint_editor_plugin`. It provides the `.class`
folder file type and `graph_save.json` marker, editor factory and panel
creation, AI tool listing/capabilities/execution, and the Blueprint scripting
language. It also syncs the open graph into the AI session. The plugin itself
implements the corresponding editor, AI, component, subsystem and scripting
traits in `plugins/vendor/blueprint_editor/src/lib.rs`. No component or
subsystem definitions are contributed today. This is the capability list to
preserve when removing the wrapper.

## Baseline commands and results

Commands run for this baseline:

```powershell
cargo test -p pulsar_script_vm
cargo test -p pulsar_world_registry
cargo test -p pulsar_class
cargo test -p pulsar_game scripting::tests
cargo test --manifest-path plugins/vendor/blueprint_editor/compiler/Cargo.toml
cargo test -p pulsar_world_registry --test component_dispatch --release baseline_property_reads -- --ignored --nocapture
```

Results: `pulsar_script_vm` passed (53 tests plus one ignored doc test),
`pulsar_world_registry` passed (28 tests; one manual benchmark is ignored by
default), `pulsar_class` passed (24 tests), and the GPUI-free Blueprint
compiler passed (22 integration tests). The focused
`pulsar_game scripting::tests` run compiled and ran 19 tests: 18 passed, and
`script_events::a_plugin_event_reaches_a_script_handler` failed while building
its nested test plugin because the locked `thiserror 2.0.21` was unavailable
from the locally cached crates.io index (cached candidates ended at 2.0.20).
No scripting assertion failed in that run. The full Blueprint editor plugin
suite and generated-Rust debug/release execution remain unrun. The ignored
generated-project checks were not run; one compiles generated output and the
new baseline probe executes it in both profiles.

Run the executable Rust probe with:

```powershell
cargo test -p pulsar_game --test generated_project_compiles generated_rust_variable_baseline_runs_in_debug_and_release -- --ignored --nocapture
```

It uses PBGC's graph-to-Rust and project-generation APIs, then calls the
generated actor's begin-play and tick methods through the pinned `Actor` trait.
Its expected trace captures current shared thread-local variable behavior; it
is not a parity assertion against VM instance state.

Property timing and allocation results (Windows 11, local developer machine):

| Path | Release ns/access | Allocation calls/access | Command/result |
|---|---:|---:|---|
| Direct typed SceneDB read | 4.8 | 0 | `cargo test -p pulsar_world_registry --test component_dispatch --release baseline_property_reads -- --ignored --nocapture` (passed, 300,000 reads) |
| Boxed reflected read | 121.8 | 4 | same command |
| JSON reflected read | 193.7 | 6 | same command |
