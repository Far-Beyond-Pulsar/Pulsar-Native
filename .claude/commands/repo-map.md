# Pulsar Engine — Repo Map

A concise orientation to the Pulsar-Native workspace and the repositories it pulls in.

---

## Repositories

Pulsar-Native is the workspace; most other repositories are git submodules inside it.

| Repo | Where in Pulsar-Native | Role |
|------|------------------------|------|
| **Pulsar-Native** | (workspace root) | Engine runtime, editor, build tooling |
| **Plugin_Blueprints** | `plugins/vendor/blueprint_editor` | Blueprint editor plugin, plus its `compiler/` and `graph/` crates |
| **Graphy** | `crates/third-party/graphy` | Graph data model and data/exec-flow analysis |
| **Helio** | `crates/renderer/helio` | GPU-driven renderer |
| **WGPUI / WGPUI-Component** | `crates/ui/wgpui`, `crates/ui/wgpui-component` | UI framework (`gpui-ce`) and component library (`ui`) |
| **Pulsar-Reflection** | `crates/external/pulsar-reflection` | `EngineClass` reflection and derives |
| **SceneDB** | git dependency (`pulsar_scenedb`) | Entity/component world store |

Other editor plugins live beside the Blueprint editor in `plugins/vendor/`.

Submodules are pinned to commits on their upstream default branch (`just check-submodule-pins`).
To land a change in one: merge its PR, re-pin the submodule, then merge the Pulsar-Native PR. See
`/bump-deps`.

---

## Pulsar-Native Workspace Layout

```
Pulsar-Native/
├── crates/
│   ├── core/                     Engine and editor-backend crates
│   │   ├── pulsar_game/          Game runtime: TickLoop, scripting (ScriptDriver), standalone runner
│   │   ├── pulsar_std/           Standard library of Blueprint nodes (#[blueprint] functions)
│   │   ├── pulsar_macros/        #[blueprint] and the script-native generators
│   │   ├── pulsar_script_vm/     Script module format + VM (every language compiles to it)
│   │   ├── pulsar_script_runtime/ ScriptRuntime: script classes bound to entities
│   │   ├── pulsar_script_codegen/ Exports a script module as a Rust actor
│   │   ├── engine_backend/       Editor services; services/core_project_builder.rs scaffolds game projects
│   │   ├── engine_class_derive/  proc-macro: #[derive(EngineClass)]
│   │   ├── engine/               The editor binary (links the built-in plugins)
│   │   ├── engine_state/         Global engine state, project path, build configurations
│   │   ├── plugin_editor_api/    Trait API editor plugins implement
│   │   ├── plugin_manager/       Loads editor plugins
│   │   └── …
│   ├── editor/                   Editor UI crates (ui_core, ui_level_editor, ui_build, ui_file_manager, …)
│   ├── ui/                       wgpui (gpui-ce), wgpui-component (ui)
│   ├── renderer/helio/           Renderer
│   ├── external/pulsar-reflection/
│   ├── third-party/              graphy, pulsar-config, psgc, toolbelt
│   └── subsystems/, graphics/, profiling/, agent-providers/
├── plugins/vendor/               Editor plugins (submodules)
├── tools/                        Standalone CLI tools
├── scripts/                      CI checks (fmt-check.sh, plugin-pins.sh, submodule-pins.sh, …)
└── Cargo.toml                    Workspace root: [workspace.dependencies] and the [patch] tables
```

## Plugin_Blueprints Layout

```
plugins/vendor/blueprint_editor/
├── src/
│   ├── lib.rs                    Plugin entry point
│   ├── std_nodes.rs              pulsar_std node catalogue as Graphy metadata
│   ├── core/                     Node definitions, graph types, events
│   ├── editor/                   BlueprintEditorPanel
│   ├── io/                       graph_save.json formats, save/load
│   ├── features/
│   │   ├── compilation/          compiler.rs: validation, VM and Rust compile targets
│   │   ├── class_dirs.rs         Where classes live (src/classes/<Name>/, <Name>.class/)
│   │   ├── variables/, macros/, events/, traits/, prefabs/, undo/, …
│   ├── rendering/                Graph canvas
│   └── ui_components/
├── compiler/                     blueprint_compiler: graph → pulsar_script_vm::Module
└── graph/                        blueprint_graph: the saved asset and authored graph types
```

## Graphy Layout

```
crates/third-party/graphy/src/
├── core/                         GraphDescription, NodeInstance, Connection, NodeMetadata
├── analysis/                     data_flow.rs (DataResolver), exec_flow.rs (ExecutionRouting)
└── utils/                        SubGraphExpander, …
```

---

## Key Cross-Cutting Patterns

### Blueprint pipeline
`graph_save.json` → validation → `blueprint_compiler` → a script module, either loaded by the
`ScriptRuntime` (VM target) or exported to Rust by `pulsar_script_codegen` (Rust target). See
`/blueprint-system`.

### exec_output! macro
Control-flow nodes declare exec outputs with `exec_output!("Label")` in their `pulsar_std` body.
`#[blueprint]` records the labels in the node registry (`exec_outputs`), which become the node's
exec pins; non-intrinsic flow nodes run as selector natives that report which label fired.

### Generated project files
`ensure_core_bootstrap()` in `core_project_builder.rs` runs on Build Core, PIE and packaging:
- `Cargo.toml`: dependencies and the `[patch]` tables baked from the workspace by
  `crates/core/engine_backend/build.rs`
- `src/main.rs`, `src/lib.rs` (PIE entry points), `src/engine_main.rs` (`setup()`, which calls
  `enable_project_scripting()`)
- `src/classes/mod.rs`: rebuilt from disk, one `pub mod X;` per class directory with a `mod.rs`
- `Pulsar/level.json`, engine primitive meshes

### Blueprint class files
Written by the Blueprint plugin's compiler; see the layout in `/blueprint-system`.
