# Pulsar Engine — Repo Map

Print a concise orientation to every repository and crate in the Pulsar monorepo ecosystem.

---

## The Five Repositories

| Repo | GitHub | Local path | Role |
|------|--------|-----------|------|
| **Pulsar-Native** | `Far-Beyond-Pulsar/Pulsar-Native` | `~/Documents/GitHub/Pulsar-Native` | Main workspace — engine runtime, editor UI, build tooling |
| **Plugin_Blueprints** | `Far-Beyond-Pulsar/Plugin_Blueprints` | `~/Documents/GitHub/Plugin_Blueprints` | Blueprint editor GPUI plugin |
| **PBGC** | `Far-Beyond-Pulsar/PBGC` | `~/Documents/GitHub/PBGC` | Blueprint Graph Compiler — JSON graph → Rust codegen & bytecode |
| **Graphy** | `Far-Beyond-Pulsar/Graphy` | `~/Documents/GitHub/Graphy` | Graph description types + AST transformation utilities |
| **Helio** | `Far-Beyond-Pulsar/Helio` | (external) | Scene/ECS runtime used by game projects |

## Dependency Order (bottom → top)

```
Graphy  ←  PBGC  ←  Pulsar-Native  ←  Plugin_Blueprints
                          ↑
                    (workspace root)
```

Changes cascade upward. Always push the lowest-level repo first, then update `rev =` pins going up.

---

## Pulsar-Native Workspace Layout

```
Pulsar-Native/
├── crates/                    Engine crates
│   ├── pulsar_game/           ECS runtime (World, Actor, TickLoop, blueprint_runtime)
│   ├── engine_backend/        Editor services (project builder, LSP, GPU renderer)
│   │   └── services/
│   │       └── core_project_builder.rs   ← generates user project scaffolding
│   ├── blueprint_compiler/    Drives PBGC from within the editor
│   ├── pulsar_std/            Standard library of blueprint nodes (print_number, etc.)
│   ├── pulsar_std_bundle/     Bundles pulsar_std for VM executor
│   ├── pulsar_bp_executor/    Bytecode VM that runs compiled blueprints
│   ├── pulsar_reflection/     EngineClass trait + property reflection
│   ├── pulsar_reflection_derive/  proc-macro: #[derive(EngineClass)]
│   ├── engine_class_derive/   proc-macro: #[derive(EngineClass, Clone)]
│   ├── pulsar_game/           Game runtime (ECS + blueprint dispatcher)
│   ├── engine_state/          Global engine state, project path, settings
│   ├── plugin_editor_api/     Trait API all editor plugins must implement
│   ├── plugin_manager/        Loads/unloads editor plugins at runtime
│   ├── ui/                    Core GPUI components (Button, Notification, theme…)
│   └── …
│
├── ui-crates/                 Editor panel crates
│   ├── ui_core/               Root window; loads all editor plugins
│   ├── ui_level_editor/       Level editor panel + Build Core toolbar button
│   ├── ui_file_manager/       File browser
│   ├── ui_log_viewer/         Tracing log viewer
│   └── …
│
├── tools/                     Standalone CLI tools
└── Cargo.toml                 Workspace root — all [workspace.dependencies] live here
```

## Plugin_Blueprints Layout

```
Plugin_Blueprints/src/
├── lib.rs                     Plugin entry point
├── core/                      Shared types, definitions, errors
├── editor/                    Main BlueprintEditorPanel + panel.rs
├── features/
│   ├── compilation/           compiler.rs — triggers PBGC, writes events.rs
│   ├── variables/             Variable list UI (hierarchy_item, rendering)
│   ├── macros/                Local macro editor
│   └── prefabs/               Component/prefab panel
├── rendering/                 Blueprint graph canvas rendering
└── ui_components/             Reusable UI widgets for the editor
```

## PBGC Layout

```
PBGC/src/
├── lib.rs
├── compiler.rs                Top-level compile pipeline
├── disk.rs                    Read/write blueprint JSON files
├── metadata.rs                Node metadata registry (BlueprintMetadataProvider)
├── project.rs                 gen_events_rs() — the events.rs template
│                              mod logic { } template lives here
├── bytecode/                  Bytecode instruction set + serialisation
├── codegen/
│   └── rust_codegen.rs        BlueprintCodeGenerator — graph → Rust function body
└── vm/                        Bytecode VM executor
```

## Graphy Layout

```
Graphy/src/
├── lib.rs
├── core/                      GraphDescription, NodeInstance, DataResolver
├── utils/
│   └── ast_transform.rs       inline_control_flow_function + extract_exec_output_labels
└── …
```

---

## Key Cross-Cutting Patterns

### exec_output! macro
Control-flow nodes (Sequence, Branch) declare outputs with `exec_output!("Label")` in their function template. PBGC's AST transformer (`ast_transform.rs`) replaces each label with the connected chain of compiled node calls.

### Generated project files
`core_project_builder.rs::ensure_core_bootstrap()` regenerates on every Build Core press:
- `Cargo.toml` — user project manifest (no rev pins; Build Core runs `cargo update` first)
- `src/main.rs` — entry point
- `src/engine_main.rs` — VM blueprint loader
- `src/classes/mod.rs` — scanned from disk, one `pub mod X;` per class directory

### Blueprint class files (written by Plugin_Blueprints compiler)
```
src/classes/<ClassName>/
├── mod.rs           pub mod vars; pub mod events; pub use events::*;
├── vars/mod.rs      Variable storage (Cell<T>, RefCell<T>)
└── events/
    ├── mod.rs       pub mod events; pub use events::*;
    └── events.rs    #[derive(EngineClass, Clone)] struct + impl Actor
                     mod logic { use pulsar_std::*; ... generated fn begin_play() }
```
