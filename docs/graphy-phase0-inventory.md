# Graphy migration — Phase 0 inventory and performance baseline

Status: research only. No implementation code, manifests, pins or branches were
changed. Checkout: Pulsar-Native `main` @ `3ffbecda9`; submodules as pinned:
Graphy `227bde3`, Blueprint plugin `e04128e`, Shader plugin `385699b`
(`render_bps` is **not** a submodule; it lives in the main repo).
Counts below are `wc -l` / `diff` line counts on this checkout (approximate; "diff
lines" = lines starting `<`/`>` in `diff`, so it over-counts moved lines).

## 0. Discrepancies with the plan's "Research findings"

| # | Plan says | Checkout shows |
|---|---|---|
| D1 | Graphy "has no shared ... graph canvas"; its package is compiler-oriented | Correct for UI/render. But Graphy **already contains a Blueprint document model**: `src/core/document.rs` (652 LOC: `BlueprintDocument`, `Graph`, `GraphId`, `GraphKind`, `ClassVariable`, `BlueprintMetadata`, `GraphViewState`, `DocumentEditorState`), plus `GraphDescription`/`NodeInstance`/`Pin`/`Connection`/`DataType`/`Position` in `core/`. Nothing outside Graphy uses `BlueprintDocument`/`GraphKind`/`DocumentEditorState` (grep across `crates/` and `plugins/`; hits are only Graphy itself and a few same-named local types in the Blueprint plugin). So there are **three** overlapping schemas: `graphy::core::*`, `blueprint_graph::*` and the Blueprint editor's own `core/types.rs` (`BlueprintNode`, `Pin`, `PinDataType`…). Phase 1's "which Graphy types become core vs compiler IR" must start from this. |
| D2 | `pulsar_game` "directly depends on the plugin-local `blueprint_graph`" | True but **dev-dependency only** (`crates/core/pulsar_game/Cargo.toml:89-90`, under `[dev-dependencies]`), used by exactly one test (`tests/random_world_blueprint.rs:10-11`, `blueprint_compiler` + `blueprint_graph::codec::deserialize_blueprint`). There is no normal/runtime dependency, no `src/` use. Impact: no engine runtime crate depends on the editor submodule through this edge; the concern is test-graph layering and the root workspace membership. |
| D3 | Shader Editor "depends on `blueprint_graph` and Graphy" | Shader's *own* document is `psgc::GraphDescription` (`ShaderAsset.main_graph`, `io/formats.rs:19`), not a `blueprint_graph` type. Its `blueprint_graph` use is **only** `ConnectionType` (≈9 refs in `ai_tools.rs`, `core/types.rs:218`, `editor/panel.rs`, `features/clipboard`, `features/compilation/conversion.rs:360`, `features/connections/operations.rs`) and `io/legacy.rs` (imports `Connection, ConnectionType, GraphDescription, NodeInstance, GraphMetadata`). Root `Cargo.toml:8` comment calls it a "Compatibility schema consumed by Shader Editor's legacy file model". `graphy::ConnectionType` already exists, so this edge is almost removable on its own. |
| D4 | Blueprint plugin "depends on Graphy" | It pins Graphy by **git rev** `c439240…` (`blueprint_editor/Cargo.toml:55`, `compiler/Cargo.toml:10`), while the submodule is at `227bde3`. Inside the root workspace the root `[patch."https://github.com/Far-Beyond-Pulsar/Graphy"]` (`Cargo.toml:462`) redirects it to the submodule path, so in-tree builds are unified; **standalone** plugin builds use a different Graphy. Same for `engine_fs` (git rev `6c61db9…`). See §5.3: the standalone plugin does not even compile today. |
| D5 | `.agents/CRATES.md` lists `pulsar_graph`, `pulsar_bp_executor` as stale | Confirmed. No directory under `crates/` and no `Cargo.toml`/`.rs` reference (the only `pulsar_graph…` hits are unrelated Helio WGSL helper names `pulsar_graph_sample*` in `helio-mats/src/shader.rs`). Stale prose rows: `.agents/CRATES.md:37,42` and `crates/core/README.md:22` (`crates/core/README.md` lists only `pulsar_graph`). `.agents/SCRIPT_VM_PLAN.md:260` records that Phase 5 removed `pulsar_bp_executor`. `SCRIPTING_EPIC.md` / `SCRIPT_VM_PLAN.md` mention them as history; leave unless the owner wants rewrites. |
| D6 | CI "explicitly enrolls/expects `blueprint_graph`" | Confirmed: `.github/workflows/ci.yml:236` `$expectedVendorWorkspaceMembers` includes `blueprint_compiler`, `blueprint_graph`, `render_bps_plugin`, …; also `ci.yml:208` (`blueprint_editor_plugin` must have `builtin`), `:260` (`shader_editor_plugin` must be a `ui_core` dependency) and `:275` (`ui_core` must not depend on `blueprint_editor_plugin`/`blueprint_compiler`). The root workspace `members` list at `Cargo.toml:7-9` carries both. |
| D7 | Graphy is a standalone upstream repo | Graphy's `Cargo.toml` declares `[profile.release]` (fat LTO, `panic=abort`, …). Inside the Pulsar workspace those are **ignored** (cargo prints "profiles for the non root package will be ignored"), so Graphy benches run with Pulsar's release profile, not Graphy's. Baselines must record which. |
| D8 | Plan frames `render_bps` as using Blueprint `EditorProfile` | True; but the profile trait's *signature* is in terms of `blueprint_graph::BlueprintAsset` (`decode`/`encode`/`validate`/`graph_applied`) and the editor's `BlueprintGraph`/`NodeDefinitions`. The contract is therefore Blueprint-shaped, not domain-neutral (see §3a). |

## 1. Ownership table

Classification: **G** move to Graphy · **D** remain domain-owned · **A** temporary
migration adapter (with removal condition) · **X** delete as stale.
"Rev deps" = reverse dependencies found by `grep` over `Cargo.toml` and `*.rs`
(excluding `target/`).

### 1.1 Graphy (`crates/third-party/graphy`, 12.4k LOC incl. tests/benches)

| ID | Item | LOC | Class | Notes / rev deps |
|---|---|---|---|---|
| G1 | `core/{graph,node,connection,metadata}.rs` (`GraphDescription`, `NodeInstance`, `Pin`, `Connection`, `NodeMetadata`) | ≈1.5k | G (stay; become `graphy-core`) | Rev deps: `blueprint_compiler`, `blueprint_editor_plugin`, `shader_editor_plugin`, `psgc` (+`wgsl_std`, `shader_tests`), `code_editor` (`plugins/vendor/code_editor/Cargo.toml:107`), root `[workspace.dependencies]` `Cargo.toml:271`. |
| G2 | `core/types.rs` (`DataType`, `TypeInfo`, `ReflectedType`, `PropertyValue`…) | 1206 | G, needs split | Mixes generic type keys with Rust-reflection shapes (`ReflectedType`, `WrapperKind`); the generic/Pulsar split is the Phase 1/2 decision. |
| G3 | `core/document.rs` (`BlueprintDocument`, v2 multi-graph) | 652 | **Undecided** (D1) | Unused outside Graphy. Either becomes the `graphy-core` document and the basis of `graphy-blueprint`, or is deleted. It overlaps `blueprint_graph::BlueprintAsset`. |
| G4 | `analysis/*`, `generation/*`, `utils/*`, `parallel.rs`, `logging.rs` | ≈2.3k | G (`graphy-compiler`) | Headless; keep free of GPUI/WGPU. |
| G5 | `type_checker.rs` (855), `diagnostics.rs` (452) | 1.3k | G | `CoercionRegistry` is explicitly *not* used by Blueprint (SCRIPTING_EPIC: "explicitly NONE"); Blueprint reuses only `Diagnostic`/`Severity`. |
| G6 | `tests/*` (15 files), `benches/{graph_benchmarks,threadpool_bench}.rs`, `examples/stress_test.rs` | ≈5k | G | 36 unit + ≈200 integration tests pass here (§5.2). No test for document.rs consumers. |
| G7 | Graphy `[profile.*]` + `Cargo.lock` | – | G (becomes workspace root files) | Profile ignored when built from Pulsar (D7). |

### 1.2 `plugins/vendor/blueprint_editor` (submodule; 62k LOC across plugin + compiler + graph)

| ID | Item | LOC | Class | Rev deps / notes |
|---|---|---|---|---|
| B1 | `graph/` package `blueprint_graph` (`lib.rs` 1476, `codec` 55, `legacy` 219, `library` 85, `prefab` 47, `type_system` 220) | 2.1k | **G** → `graphy-blueprint` (except any UI-only bits; none found: GPUI-free) | Rev deps: `blueprint_editor_plugin`, `blueprint_compiler`, `shader_editor_plugin` (ConnectionType + legacy), `render_bps_plugin`, `pulsar_game` (dev-dep). Root member (`Cargo.toml:9`) and CI list (`ci.yml:236`). |
| B2 | `graph/tests/{persistence.rs, fixtures/graph_save.json}` | 95 + fixture | G (moves with B1) | Only authored-Blueprint round-trip fixture besides `assets/examples/random_world.blueprint.json`. |
| B3 | `compiler/` package `blueprint_compiler` (`lib.rs` 2581, `authored.rs` 241, `palette.rs` 258; tests 2568+119) | 5.8k | D (Blueprint → script module lowering) | Uses `graphy::{ConnectionType,DataType,GraphDescription,NodeInstance}` + `pulsar_script_vm`. Rev deps: `blueprint_editor_plugin`, `pulsar_game` (dev), root member; also referenced by `pulsar_script_conformance`, `pulsar_script_ts` (comments/fixtures only). |
| B4 | `src/rendering/gpu/{renderer 800, text 697, types 106, delta 99}` + 8 WGSL (`angular` 261, `bezier` 114, `comments` 82, `grid` 75, `nodes` 168, `pins` 173, `text` 53, `wires` 82) | ≈2.7k | **G** (`graphy-render-wgpu`) | Domain-neutral already apart from pin/node style tables. |
| B5 | `src/rendering/{graph 1791, graph/geometry 398, graph/context_menu 244, input 634, input/hit_testing 374, layout 60, overlay 335, style 73}` | ≈3.9k | **G** (`graphy-gpui` / `graphy-render`) except context-menu content (D) | `hit_testing.rs` is the only Blueprint hit-test file; layout lives in `rendering/layout.rs` (60) + `graph/geometry.rs`. |
| B6 | `src/core/spatial_index.rs` | 1109 | **G** (`graphy-render`) | Has the only existing headless benchmark (§5.1). |
| B7 | `src/core/{graph 153, graph_entity 103, serialization 116, events 73, types 569, definitions 706, subgraph_ref 94}` | 1.8k | Mixed. `graph`, `graph_entity`, `serialization`, `events`, `types` (node/pin/connection/comment editor model) → **G** (collapses with the document, see §4.3). `definitions` (node catalogue loader) → D. `subgraph_ref` → D. |
| B8 | `src/features/{viewport, nodes, connections, comments, clipboard, undo, validation}` | ≈3.6k | Generic parts → **G** (selection/connect/clipboard/undo-as-commands); Blueprint rules (`compatibility.rs`, `validation.rs`, trait contracts) → D | See duplication §2. |
| B9 | `src/features/{variables, events, macros, prefabs, traits, class_dirs, compilation, debug, script_problems}`, `ai_tools.rs` (1108), `ui_components/*`, `editor/*` (shell, toolbar, panels), `io/*`, `std_nodes.rs`, `builtin_provider.rs`, `lib.rs` | ≈26k | D (editor shell + Blueprint semantics) | The editor shell (`editor/panel.rs` 1673, `workspace_panels.rs` 1393, `panel_render.rs`, `toolbar.rs`) is the *other* big duplicate vs Shader (see §2) but is chrome, which the plan keeps out of Graphy; candidate for a later shared "editor-shell" decision, not Phase 3. |
| B10 | `src/profile.rs` (`EditorProfile`, `PinShape`, `PinStyle`, `ParamField`, `install_profile`) | 214 | **G** (`graphy-gpui` profile API), after de-Blueprinting (D8) | Rev deps: `render_bps` only. Global `static PROFILES` registry: must not become a cross-DLL static (see Risks). |
| B11 | `examples/{standalone, standalone_stress, spatial_index_stress}.rs` | 46 / 555 / 243 | G for harness logic; **standalone examples must be re-homed** | See §5. |
| B12 | `Cargo.toml` features: `builtin = ["dep:plugin_manager"]` | – | D | Required by `ci.yml:208`. |
| B13 | Blueprint `Cargo.lock`, `out.tree` (checked-in dependency tree dump) | – | `out.tree`: X candidate (not referenced by any manifest; generated artefact — verify with the plugin owner before deleting). |

### 1.3 `plugins/vendor/shader_editor` (submodule; 17.8k LOC)

| ID | Item | LOC | Class | Notes |
|---|---|---|---|---|
| S1 | `rendering/gpu/{renderer 917, text 624, types 120, delta 99}` + 7 WGSL (`bezier` 114, `comments` 82, `grid` 75, `nodes` 149, `pins` 132, `text` 53, `wires` 82) | ≈2.4k | **G** (duplicate of B4; delete after migration) | 4 WGSL files are byte-identical to Blueprint's (`comments`, `grid`, `text`, `wires`); `bezier`, `nodes`, `pins` differ. |
| S2 | `rendering/gpu/pin_preview.rs` + `shaders/texture_previews.wgsl` | 349 + 63 | D, expressed through a Graphy per-pin extension hook | Texture preview. |
| S3 | `rendering/{graph 1713, input 682, layout 162, overlay 116, style 73}` | ≈2.7k | **G** (dup of B5) | Hit-testing is embedded in `input.rs:37-180`. |
| S4 | `core/{graph 82, graph_entity 103, serialization 116, events 89, types 464, definitions 268}` | 1.1k | graph_entity/serialization/events/types → **G**; definitions → D | |
| S5 | `features/{viewport, nodes, connections, comments, clipboard, undo}` | ≈2.4k | Generic → **G**; `compatibility.rs` (164) shader type rules → D | |
| S6 | `features/compilation/{compiler 289, conversion 715, panel 53}` | 1.1k | D (PSGC path, §3b) | |
| S7 | `features/preview/*`, `shader_model/*`, `import_companion.rs`, `ai_tools.rs`, `ui_components/*`, `editor/*`, `io/*` | ≈10k | D (shell/previews/format) | |
| S8 | `io/legacy.rs` (289) | 289 | **Delete after merge** with B1 `graph/src/legacy.rs` (219): same `Legacy*` types and `try_parse_legacy_format`, differing only in the target graph type (`psgc::GraphDescription` vs `blueprint_graph::GraphDescription`). | Rev dep: `io/save_load.rs`. 4 tests. |
| S9 | `Cargo.toml` `blueprint_graph = { path = "../blueprint_editor/graph" }` | – | X (after D3 is applied: swap to `graphy::ConnectionType`) | Cross-plugin path dep between two submodules. |

### 1.4 `plugins/vendor/render_bps` (main repo, 2.2k LOC) and Helio

| ID | Item | LOC | Class |
|---|---|---|---|
| R1 | `src/model.rs` | 1101 | **A**, then largely D (see §3a) |
| R2 | `src/lib.rs` (`RenderProfile`), `buffers_panel` 190, `sections_panel` 148, `preview` 398, `builtin_provider` 82 | 1.1k | D |
| R3 | `Cargo.toml` deps `blueprint_editor_plugin` + `blueprint_graph` | – | Replace with Graphy canvas/profile + schema deps |
| R4 | `crates/renderer/helio/crates/helio-graph-asset` (asset, layout, pins, buffers, sections, validate, resources) | – | D (Helio-owned; has its own `GraphAsset` schema, `FORMAT_VERSION`, unit tests) |
| R5 | `plugins/vendor/asset_viewer/src/editor/graph_material*.rs` | – | D, not a canvas (confirmed: preview only) |

### 1.5 Main repo, manifests, CI, docs

| ID | Item | Class | Action when its consumer moves |
|---|---|---|---|
| M1 | Root `Cargo.toml:7-9` members `blueprint_editor/compiler`, `blueprint_editor/graph` | A | `graph` entry removed when `graphy-blueprint` lands; `compiler` entry stays (test against this repo's VM) unless compiler moves to Graphy. |
| M2 | Root `Cargo.toml:60,63` excludes `blueprint_editor`, `shader_editor` | D | Unchanged; they are separate repos. |
| M3 | Root `[workspace.dependencies] graphy` (`:271`) + `[patch…Graphy]` (`:462-463`) | A | Needs one entry per new Graphy package (path form), or a patch per package; remove the git-rev duplicates in plugin manifests (D4). |
| M4 | `crates/core/pulsar_game/Cargo.toml:89-90` | A | Repoint to `graphy-blueprint` (+ compiler). Dev-only. |
| M5 | `crates/core/pulsar_package` optional `blueprint_editor_plugin` (builtin); `crates/core/engine` optional `blueprint_editor_plugin` (`blueprint` feature) and `render-graph` feature | D | Link-time `use blueprint_editor_plugin as _;` (`engine/src/main.rs:49`, `pulsar_package/src/compile.rs:220`). |
| M6 | `crates/editor/ui_core` dep `shader_editor_plugin` (`Cargo.toml:53`, `builtin_editors.rs`) | D | Required by `ci.yml:260`. |
| M7 | `.github/workflows/ci.yml:208,236,260,275` | A | Edit `:236` list when `blueprint_graph` leaves the workspace; add Graphy workspace test job. |
| M8 | `.agents/CRATES.md:37,42` stale `pulsar_bp_executor`, `pulsar_graph`; `crates/core/README.md:22` | **X** (both proven absent, D5) |
| M9 | `.agents/CRATES.md:200` ("`graphy/` Graph data model and compiler types"), `:210-212` plugin list | A | Rewrite for the workspace layout. |
| M10 | `.agents/RENDER_GRAPH.md:20,162,194-197` (profile API location, `to_blueprint_asset`) | A | Update with Phase 5. |
| M11 | `psgc` (`crates/third-party/psgc`, submodule) `graphy = { workspace = true }`; `wgsl_std` node macros use `graphy::` types | D | Consumer of Graphy compiler API; a Graphy rename touches ≈15 `wgsl_std` files. |
| M12 | `plugins/vendor/code_editor` `graphy = { path }` (`Cargo.toml:107`) | ? | Unverified use; grep finds no `graphy::` in `src/`. Likely an unused dependency: check with `cargo +nightly udeps` or a manual build before removing. Not proven unused. |

## 2. Duplication map (Blueprint vs Shader plugin)

Paths relative to `src/` in each plugin. "Identical" is `cmp`/`diff` empty.

| Area | Blueprint | Shader | Verdict |
|---|---|---|---|
| `core/graph_entity.rs` | 103 | 103 | **Identical** |
| `core/serialization.rs` | 116 | 116 | **Identical** |
| `rendering/style.rs` | 73 | 73 | **Identical** |
| `rendering/gpu/delta.rs` | 99 | 99 | **Identical** |
| WGSL `comments`, `grid`, `text`, `wires` | 82+75+53+82 | same | **Identical** (292 each side) |
| `core/events.rs` | 73 | 89 | ≈32 diff lines → near-identical |
| `editor/undo_ops.rs` | 82 | 82 | 14 diff lines |
| `features/comments/operations.rs` | 20 | 20 | 6 diff lines |
| `features/undo/mod.rs` | 702 | 652 | 52 diff lines; same command/stack design |
| `features/viewport/{coordinates,operations}.rs` | 137+79 | 149+88 | 26 / 45 diff lines; this is the screen↔graph transform |
| `features/connections/compatibility.rs` | 139 | 164 | 27 diff lines but *domain rules* (D) |
| `features/nodes/selection.rs` | 207 | 176 | 35 diff lines |
| `rendering/gpu/text.rs` | 697 | 624 | 119 diff lines; glyph atlas/text renderer |
| `core/types.rs` | 569 | 464 | 175 diff lines; Pin/Node/Connection/Comment model; shader adds `PinDataType` texture rules (`core/types.rs:82`) |
| `features/clipboard/mod.rs` | 299 | 314 | 197 diff lines |
| `features/nodes/operations.rs` | 331 | 261 | 108 diff lines |
| `features/connections/operations.rs` | 496 | 470 | 316 diff lines (shader differences are mostly type checks) |
| `rendering/input.rs` (+ hit testing) | 634 + 374 | 682 | 510 diff lines; same state machine, hit-test code is in separate file for Blueprint, inline for Shader |
| `rendering/graph.rs` (canvas, culling, draw-list build) | 1791 | 1713 | 2220 diff lines: divergent in structure but same job (see below) |
| `rendering/gpu/renderer.rs` | 800 | 917 | 1717 diff lines: divergent; Shader adds texture preview pass |
| `rendering/gpu/types.rs` | 106 | 120 | 226 diff lines |
| WGSL `bezier`, `nodes`, `pins` | 114 / 168 / 173 | 114 / 149 / 132 | `bezier` differs slightly; the other two differ in node/pin styling; Blueprint additionally has `angular.wgsl` (261) |
| `rendering/layout.rs` | 60 | 162 | Shader owns pin-row/texture-preview height maths (`pin_row_height`, `pin_row_offset`, `node_height_for_pins`); Blueprint uses `node_height_for_pin_rows` + `graph/geometry.rs` (398). **Two different layout models.** This is the Phase 3 "one layout model" risk. |
| `rendering/overlay.rs` | 335 | 116 | Blueprint more elaborate (context menus, minimap-style helpers) |
| `editor/{panel,workspace,workspace_panels,panel_render,toolbar,entity_ops,tabs}.rs` | 1673+266+1393+735+449+384+72 | 1013+207+1054+681+385+298+35 | The editor *shell* is also copied (≈5k each side; 148–2447 diff lines each). Chrome, so outside the plan's Graphy scope, but it is the largest remaining duplicate after Graphy takes the canvas. Flag for a decision. |
| `io/legacy.rs` | `graph/src/legacy.rs` 219 | 289 | Same `Legacy*` structs, different target type |

Totals (Rust + WGSL, one copy counted): byte-identical ≈ 391 Rust + 292 WGSL;
near-identical (< ~60 diff lines) ≈ 1.8k; "same job, diverged" canvas
(rendering/graph, gpu/renderer, input, hit-testing, layout, overlay, text,
types, spatial) ≈ 7.7k Blueprint / ≈ 6.6k Shader (+ Blueprint-only
`spatial_index.rs` 1109). Reasonable Graphy-side target for one canvas stack:
the larger of the two minus domain parts, i.e. about 7–8k LOC, replacing about
14k today (a ≈6k net reduction in the canvas stack alone), before counting the
generic feature modules (≈3k each side) and shell duplication. Treat as an
estimate; LOC is not the success metric.

Domain-specific (stay): Blueprint — angular wires shader, collapsed/macro
nodes, variable/event/trait/prefab UI, compiler glue; Shader — texture-preview
pass, PSGC conversion, shader-model/material preview, type-compat rules for
WGSL.

## 3. Trace notes

### 3a. `render_bps` → Blueprint profile/canvas

Calls (all in `plugins/vendor/render_bps/src`):
- `lib.rs:16-17` imports `blueprint_editor_plugin::profile::{install_profile, EditorProfile, ParamField, PinStyle}`, `BlueprintEditorPanel`, `BlueprintGraph`; `RenderProfile` implements `EditorProfile`:
  `catalog` (→ `model::catalog`), `decode` (`GraphAsset::from_json` → `model::to_blueprint_asset`), `encode` (`model::from_blueprint_asset(...).to_json()`), `pin_style`, `parameters`, `parameter_changed` (→ `model::refresh_pins`, mutates the editor `BlueprintGraph`), `auto_layout` (→ `model::auto_layout_graph`), `left_panels` (buffers + sections panels hold a `WeakEntity<BlueprintEditorPanel>` and call `add_profile_node`), `preview_panel`, `graph_applied`, `validate` (→ `model::buildable_asset` → `model::problems`).
- `preview.rs:119` and `lib.rs:148` call `model::buildable_asset`.

Is `model.rs` a necessary adapter or a second editable graph? **Both.**
- *Necessary:* `helio_graph_asset::GraphAsset` (Helio-owned, versioned `.rendergraph`) has typed `NodeInstance` params, `SectionPort`s and its own comments, while the editor edits `blueprint_graph::BlueprintAsset` (string pin types, `NodeInstance` properties as strings). `pin_type_string`, `param_from_text`/`param_string`, `to_blueprint_asset` (l.420) and `from_blueprint_asset` (l.552) translate between them at the load/save boundary; `bypass_reroutes` (585) is a real format rule.
- *Second copy:* `live_asset` / `asset_from_live` (657-768) and `buildable_asset` (627) rebuild a **whole `GraphAsset` from the live editor graph on every validate/preview/apply**; `refresh_pins` and `auto_layout_graph` mutate the editor graph and convert again. So an edit is represented in three forms simultaneously (editor `BlueprintGraph`, `BlueprintAsset`, `GraphAsset`). Per the plan's rule 4 the target is one in-memory Graphy document with `GraphAsset` conversion **only** at load/save and for the compile/validate call, which already needs a `GraphAsset` for `helio_graph_asset::validate`. `model.rs` has 7 unit tests; they are the equivalence tests to keep.
- Format versioning: `.rendergraph` carries `format_version` (`helio-graph-asset/src/asset.rs`, rejects newer; test `round_trips_and_rejects_newer_formats`). The `template: "deferred"` empty-graph path is handled in `decode`.

### 3b. Shader graph: custom types, dynamic pins, texture previews, PSGC path

- Document: `ShaderAsset { format_version, main_graph: psgc::GraphDescription, editor_state, shader_metadata }` (`io/formats.rs:19`). Legacy: `io/legacy.rs` (`try_parse_legacy_format`, 4 tests) → `psgc::GraphDescription`.
- Editor model: its own `BlueprintGraph`/`BlueprintNode`/`Pin`/`PinDataType` in `core/types.rs`. `PinDataType` treats `TextureSrc` and `texture_*` as texture types (`core/types.rs:82`) and has `wildcard()` / `is_compatible_with` (94). **Dynamic pins** in Shader are typeless *wildcard* pins that take their type on connection (`core/types.rs:410-416`); there is no pin-count dynamic mechanism like `render_bps`'s `refresh_pins`. Blueprint's dynamic pins come from macro/collapsed interfaces and `refresh_pins` in render_bps.
- Texture previews: `rendering/layout.rs:33-100` (`texture_previews_enabled`, `pin_supports_texture_preview`, `node_has_texture_preview`, `pin_row_height`) alter row heights → **layout depends on preview presence**, so the shared layout model needs a per-row height/extension hook, not just fixed rows. Drawing: `rendering/gpu/pin_preview.rs` (349) + `texture_previews.wgsl` (63).
- Compile path: `features/compilation/compiler.rs` `compile_to_wgsl` → `main_graph_snapshot` → `compile_blueprint` (editor graph → `psgc::GraphDescription` via `features/compilation/conversion.rs`, 715 LOC, which also maps `PsgcConnectionType` ↔ `blueprint_graph::ConnectionType` at l.360) → PSGC (uses Graphy analysis + `wgsl_std` node metadata) → WGSL string → `validate_wgsl` (naga/wgpu parse, 2 tests) → material preview. Shader editor therefore converts editor graph → PSGC graph on every compile (whole-document copy) — same pattern as `render_bps`.
- PSGC is a separate workspace/submodule that depends on Graphy via `workspace = true`; Graphy API renames hit `wgsl_std` (≈15 node files) and `psgc/{compiler,codegen,metadata}.rs`.

### 3c. Main-repo consumers

`blueprint_graph`: `blueprint_editor_plugin`, `blueprint_compiler`, `shader_editor_plugin` (ConnectionType/legacy), `render_bps_plugin`, `pulsar_game` (dev-dep, one test). Examples `standalone*`/`spatial_index_stress` use `blueprint_graph::ConnectionType`.
`blueprint_compiler`: `blueprint_editor_plugin`, `pulsar_game` (dev-dep, same test); root member.
`graphy`: `blueprint_compiler`, `blueprint_editor_plugin` (git rev), `shader_editor_plugin` (path), `psgc` family, `code_editor` (declared; no `graphy::` use in src — M12), root workspace dep.
`blueprint_editor_plugin` (link-time): `crates/core/engine` (`blueprint` feature), `crates/core/pulsar_package` (`blueprint` feature), `render_bps_plugin`.
`shader_editor_plugin`: `crates/editor/ui_core` (unconditional dep, `builtin_editors.rs`).
CI: see D6. `pulsar_game`'s edge is dev-only (D2); the "engine → editor submodule" concern is real in the manifest (`pulsar_game` tests need the plugin checkout present) but no runtime edge exists.
Other "graph" crates verified **not** in scope: `ui_flamegraph`, collision-layer diagrams, widget tree, Helio frame graph / `helio-core/src/graph`, `asset_viewer` graph_material.

## 4. Compatibility fixtures (§4.1) and gaps (§4.2)

### 4.1 Located

| Asset | Fixture / test | Notes |
|---|---|---|
| Blueprint authored v1 | `blueprint_editor/graph/tests/fixtures/graph_save.json`, `graph/tests/persistence.rs` (4 tests: v1 round trip incl. identity/defaults/macros/events/layout, rename/reorder keeps variable IDs, legacy pin maps/comment objects readable, malformed pins/versions/IDs reported) | Passes here (`cargo test -p blueprint_graph`: 3 unit + 4 integration). |
| Blueprint example | `assets/examples/random_world.blueprint.json`, used by `pulsar_game/tests/random_world_blueprint.rs` (compiles + runs on the VM) | |
| Blueprint compile | `blueprint_compiler/tests/{compile.rs (2568), authored.rs (119)}` | Source-of-truth for lowering; not run here (needs the root VM build). |
| Blueprint io | `blueprint_editor/tests/io_formats.rs` (8 tests: defaults, header comments, legacy connection/HSL/hex colors, prefab UUID slots) | |
| Blueprint compiled outputs | `pulsar_script_conformance/fixtures/bp_*.module.json`, `pulsar_package/tests/fixtures/smoke_project/src/classes/*/events/.build/module.json`, `pulsar_package/tests/blueprint_compile.rs` | Downstream outputs, useful as regression oracles. |
| Shader | Unit tests in `shader_editor/src/io/{legacy,save_load}.rs`, `features/compilation/{compiler,conversion}.rs`, `import_companion.rs` (6), `node_library.rs` (9) | **No `.shader`/`.material` graph file fixtures exist on disk** (no file matches `*.shadergraph`, `*.matgraph*`; only code-embedded samples). |
| Render graph | `helio-graph-asset` unit tests (asset round trip + newer-format rejection, layout, pins, buffers, sections, validate); `render_bps/src/model.rs` 7 tests | **No `.rendergraph` file fixture** under the repo; templates are generated by `helio_default_passes::graphs::deferred_graph_asset`. |
| Graphy | `tests/serialization.rs` (JSON of `GraphDescription`), `tests/metadata.rs`, … | No v2 `BlueprintDocument` fixture. |

### 4.2 Gaps to fill before Phase 4–6
1. A matrix of real saved Blueprint files across versions (pre-unified legacy `graph_save.json`, v1 current, with macros/collapsed/prefab) — only one fixture today. Need golden-bytes "no ID churn" assertion, not just semantic round trip.
2. Shader/material graph golden files (current + legacy), incl. texture and wildcard pins, plus WGSL golden output for a few graphs.
3. `.rendergraph` golden files (deferred template, with sections and buffer nodes, disabled/bypassed nodes, reroutes, comments).
4. A test that the Shader and Blueprint `io/legacy.rs` accept the same legacy bytes (guards the merge in S8).
5. Editor-state/view metadata (pan/zoom/tabs) round trip separate from authored data.

## 5. Baseline plan and measurements

### 5.1 Existing harnesses (found)

| Harness | Scope | Headless | Measures |
|---|---|---|---|
| `blueprint_editor/examples/spatial_index_stress.rs` | 1k/5k/10k nodes, 3 wires/node, with/without long wires; per-query candidates and time vs linear scan; single-node drag update vs full rebuild | Yes (no GPU/window) | **Hit-test / spatial query latency**; no pass/fail |
| `blueprint_editor/examples/standalone_stress.rs` | ≈2,000 nodes / 5,000 connections (200×120 grid by default) in the real editor window | **No** (GPUI + GPU) | Visual only: pan/zoom by hand; no timing counters exported (it reads `VirtualizationStats`) |
| `graphy/examples/stress_test.rs`, `graphy/benches/{graph_benchmarks,threadpool_bench}.rs` | 100 → 40,000 node grids: (de)serialization, data-flow, exec routing | Yes | **Compile-pipeline latency** (Graphy side) |
| `blueprint_compiler/tests/compile.rs` | Correctness, no timing | Yes | none |
| Shader Editor | `examples/standalone.rs` only; **no stress or timing harness** | – | none |
| `render_bps`, `helio-graph-asset` | No benchmark | – | none |
| GPU timing | No GPU timestamp query or frame-time counter in either graph renderer found (`grep` for `timestamp`/`QuerySet` in both `rendering/` only finds `timestamp_writes: None` on render passes) | – | none |

Not covered by anything today: CPU frame-prep time, GPU draw time, pan/zoom FPS, memory, for either renderer; hit-test latency for Shader; compile latency of Blueprint (`blueprint_compiler::compile`) and PSGC.

### 5.2 Headless measurements taken

Machine: AMD Ryzen 9 9950X3D (16C/32T), 62 GB RAM; Windows 11 10.0.26200; GPUs present: NVIDIA GeForce RTX 4090 (driver 32.0.16.1742), AMD Radeon iGPU (32.0.13018.6) (GPU unused by these runs); `rustc 1.99.1`, `cargo 1.99.1`; Pulsar-Native `3ffbecda9`, Graphy `227bde3`; sibling checkouts as above.

**Graphy `examples/stress_test.rs`, `cargo run --release`, run from the Pulsar workspace** (root release profile because Graphy's own profile is ignored — D7; single run, no repetition, so these are *indicative*, not median/p95):

| Grid (nodes) | Serialize | Deserialize | Data-flow (seq) | Data-flow (par) | Exec routing |
|---|---|---|---|---|---|
| 10×10 (100) | 0.21 ms | 0.38 ms | 0.20 ms | 1.36 ms | 4 µs |
| 50×50 (2,500) | 3.35 ms | 8.38 ms | 7.44 ms | 11.4 ms | 17 µs |
| 100×100 (10,000) | 14.7 ms | 37.5 ms | 42.6 ms | 56.7 ms | 79 µs |
| 200×200 (40,000; 92,800 conns) | 72.5 ms | 213.7 ms | 249.8 ms | 273.8 ms | 269 µs |

Observation: parallel data-flow is slower than sequential up to 40k nodes here (thread-pool overhead), relevant to the compile-latency baseline.

**Tests (headless, `cargo test -p blueprint_graph -p graphy`, debug, separate target dir):** `blueprint_graph` 3 + 4 pass; `graphy` 36 unit tests and the integration suites shown pass (`ast_transform` 22, `code_generation` 14, `connection` 12, `core_types` 30, `data_flow` 22, `edge_cases` 23, `exec_flow` 13, …; output was truncated after `exec_flow` by my filter, no failures seen).

**Blocked: `spatial_index_stress`.** `cargo run --release --example spatial_index_stress` inside `plugins/vendor/blueprint_editor` (the only way to build an example of an excluded package) does not compile: its own `Cargo.lock` resolves `helio-core` from `Pulsar-Native` git rev `6c61db9` which fails against the resolved `wgpu` (`E0433 SubmitItem`, `E0425 ReusableCommandBuffer`, 11 errors in `helio-core/src/graph/execution.rs`). The Blueprint plugin therefore **cannot be built standalone today**, and its examples (including the only hit-test benchmark) cannot be run from the root workspace either because the package is `exclude`d. Not fixed (out of scope). Needs a decision (Open decision O3).

### 5.3 Reproducible procedure (for the user and later phases)

Use one fixed machine, `--release`, closed apps, fixed DPI/resolution, high-performance GPU forced for the process, record: CPU/GPU/driver, `rustc`, Pulsar and submodule SHAs, Cargo profile (root `[profile.release]` unless stated), window size, DPI scale, and whether `RUST_LOG`/Tracy are on. Every metric: ≥ 30 timed iterations after 5 warm-ups, report median and p95 (and min/max), never a single run.

Graph generator (same for all three sizes and both editors): deterministic LCG seed (as in `spatial_index_stress.rs`), column/row grid layout, 3 wires per node, 1-in-20 long wires, mixed pin types, 5% comments, a few reroutes; 100 / 1,000 / 10,000 nodes (≈300 / 3,000 / 30,000 connections). Save it as a fixture once (Blueprint `graph_save.json`-form, Shader `ShaderAsset`, `.rendergraph`-form) so before/after runs load identical files.

| Metric | How | Status |
|---|---|---|
| Compile latency — Graphy | `cargo run --release --example stress_test` (above), plus `cargo bench -p graphy` | **Done (single run)**; repeat for median/p95 |
| Compile latency — Blueprint | time `blueprint_compiler::compile` on the generated fixtures from a test/bench in the root workspace | Needs a new harness (Phase 1/3 prep); not present |
| Compile latency — Shader | time `compile_to_wgsl` / PSGC on generated graphs | Needs new harness |
| Hit-test latency | Blueprint: `spatial_index_stress` (needs O3 to run); Shader: no harness; later a shared harness on the Graphy layout/index crate | Blocked / needs harness |
| CPU frame prep | wrap canvas `paint`/draw-list build (`rendering/graph.rs`) in a `profiling::scope!`; read via the Tracy/flamegraph already wired through `crates/profiling` | **Manual**, needs instrumentation patch |
| GPU draw time | wgpu timestamp queries around the graph passes (none exist; add behind a flag) | **Manual**, needs instrumentation patch |
| Pan/zoom FPS | scripted (or manual constant-speed) pan over the 10k graph in `standalone_stress` / equivalent Shader example; log frame times | **Manual (user)** |
| Memory | process RSS/private bytes and GPU memory (Task Manager / `nvidia-smi`) after load and after 60 s of pan/zoom; CPU side via the Memory Explorer (`.agents/MEMORY.md`) | **Manual (user)** |

**What still needs a manual GPU run by the user:** pan/zoom FPS, GPU draw time, CPU frame prep, GPU/CPU memory for Blueprint and Shader at 100/1k/10k, plus screenshots (normal, hover, selection, connection-drag, low/high zoom) for Phase 3 acceptance. I did not launch any GUI.

## 6. Proposed design

### 6.1 Crate dependency graph

```text
graphy-core ─────────────┬─ graphy-compiler ─ (graphy-blueprint lowering helpers stay in blueprint_compiler)
 (ids, graph doc, types,  │
  commands/change sets)   ├─ graphy-blueprint  (BlueprintAsset schema, codecs, legacy, library, prefab;
                          │                     = today's blueprint_graph; deps: core, serde, chrono, uuid)
                          ├─ graphy-reflection (core + pulsar_reflection; resolves names at the host edge)
                          └─ graphy-render ─ graphy-render-wgpu ─ graphy-gpui
                              (layout, transforms,    (device/queue injected)   (canvas, input→commands,
                               spatial index, cull)                              EditorProfile successor)
Consumers (domain plugins):
  blueprint_editor_plugin → graphy-gpui, graphy-blueprint, graphy-reflection, blueprint_compiler
  shader_editor_plugin    → graphy-gpui, graphy-core (+ psgc, wgsl_std)
  render_bps_plugin       → graphy-gpui, graphy-core, helio-graph-asset
  pulsar_game (dev)       → graphy-blueprint, blueprint_compiler
  psgc / wgsl_std         → graphy-compiler (+ graphy-core)
  animation (later)       → graphy-gpui, graphy-core
```
Headless consumers (`psgc`, `blueprint_compiler`, `pulsar_game`) must build with no GPUI/WGPU: enforce with a CI `cargo tree -p blueprint_compiler -e normal | grep -E "wgpu|gpui"` negative check.

Opinion on the plan's crate list: `graphy-core` + `graphy-compiler` should probably **not** be split from today's `graphy` package in Phase 1 (compiler consumers are ≈25 files in psgc/wgsl_std); keep package `graphy` as the compiler/core owner and add the new crates around it (the plan allows either).

### 6.2 File ownership / deletion map (target)

- Delete after migration (Blueprint): B4, B5 (except context-menu content and Blueprint-only angular/collapsed bits that become hooks), B6 (moves), B7 (graph/graph_entity/serialization/events/types), generic parts of B8; `graph/` package B1 (moves to `graphy-blueprint`).
- Delete after migration (Shader): S1 (except S2 hook), S3, S4 (graph/graph_entity/serialization/events/types), generic S5, S8, S9.
- `render_bps`: delete `Cargo.toml` deps on `blueprint_editor_plugin`/`blueprint_graph`; shrink `model.rs` to boundary conversion + catalogue (the `live_asset`/`asset_from_live` re-materialisation goes away if validate/preview consume the Graphy document directly).
- Main repo: remove `blueprint_graph` from root members and CI list; fix M7–M10; drop plugin-level git-rev `graphy`/`engine_fs` pins in favour of a single revision story.
- Stale docs: M8 rows now (safe, no references).

### 6.3 Expected reductions (estimates, to be re-measured at Phase 7)

| Item | Estimate |
|---|---|
| Canvas stack (render/layout/input/hit-test/spatial/gpu/WGSL) | ≈14k LOC today across the two plugins → ≈7–8k in Graphy (≈6k net) |
| Generic feature modules (viewport/select/connect/clipboard/undo) | ≈3k per plugin → ≈3k once (≈3k net) |
| Duplicate models (graph_entity, serialization, events, types) | ≈1.1k per plugin → shared (≈1.1k net) |
| Legacy codec | −289 (Shader copy) |
| Dependencies | `shader_editor` → `blueprint_editor/graph` path dep removed; `render_bps` → `blueprint_editor_plugin` removed (stops linking the whole Blueprint editor and its `builtin` coupling into the render-graph plugin); root `blueprint_graph` member removed; the `[patch]`/git-rev duplication for `graphy`/`engine_fs` reduced to one |
| Not reduced | Editor shells (≈5k per plugin) unless a separate shell decision is made |

### 6.4 Risks

1. **Two layout models** (Blueprint fixed rows vs Shader variable-height texture rows). A single model needs per-row height hooks; hit-test and draw must derive from it (Phase 3 requirement) — the largest design risk.
2. **Profile API is Blueprint-shaped** (D8) and holds a `static PROFILES`/`CATALOGS` registry with `Box::leak`. Plugins are cdylibs: a Graphy registry static duplicated per DLL would silently split. Registry should live host-side or be passed explicitly; do not rely on `TypeId`.
3. **Plugin standalone builds are broken** (§5.2) and plugin manifests pin old Pulsar/Graphy git revs; any Graphy workspace conversion must update every pin consistently and the dependency-first merge order in `.agents/CRATES.md`.
4. **Three existing schemas** (D1): choosing the wrong core model before the `BlueprintDocument` question is answered would add a fourth.
5. **Whole-document conversion on compile/preview** exists in Blueprint (`conversion.rs`, 539), Shader (`conversion.rs`, 715) and `render_bps` (`live_asset`); removing it changes undo/dirty semantics — needs command/change-set design first.
6. **PSGC/`wgsl_std` fan-out** if Graphy package or type names change.
7. **No render/golden/perf baselines exist for Shader**; regressions would be invisible without the harness work in §5.3.
8. **Graphy profile ignored in-workspace** (D7): benchmarks must say which profile they use.
9. **Shared editor shell duplication** is left over after the plan and will keep drifting.
10. Workspace-member/CI assumptions (`ci.yml:236`) fail loudly if `blueprint_graph` moves without the same-PR edit.

### 6.5 Proposed ordered migration sequence (repository-sized changes)

0. **Pulsar-Native (docs-only, safe now):** drop stale `pulsar_graph`/`pulsar_bp_executor` rows (M8) after owner OK.
1. **Pulsar-Native:** add the benchmark/instrumentation harness: deterministic graph generator + fixtures (§4.2 items 1–3), `profiling::scope!` + GPU timestamp hooks in both renderers (touches plugin submodules → each plugin PR), record baselines on the user's machine.
2. **Plugins (Blueprint, Shader) housekeeping PRs:** Shader: replace `blueprint_graph::ConnectionType` with `graphy::ConnectionType` (D3), drop the cross-plugin path dependency; fix Blueprint standalone build (O3).
3. **Graphy PR 1:** convert to a workspace (`graphy` stays; add empty-ownership-free skeleton only when each crate has code), decide D1 (document model), add `graphy-blueprint` by **moving** `blueprint_graph` (+ its tests/fixtures) with git history; workspace tests; CI headless-dependency guard.
4. **Pulsar-Native PR A:** repoint `pulsar_game`, `blueprint_compiler`, `render_bps`, root members, `ci.yml:236`, lockfile, `[patch]`; bump Graphy pin. (Blueprint plugin PR repoints at the same time — dependency-first order.)
5. **Graphy PR 2:** `graphy-reflection` adapter (needs Pulsar-Reflection first if new APIs are required).
6. **Graphy PR 3:** `graphy-render` — layout model (with row-height hook), transforms, spatial index (from B6), hit-test; tests; run the hit-test harness.
7. **Graphy PR 4:** `graphy-render-wgpu` + `graphy-gpui` with the redesigned visuals (Phase 3), screenshots/goldens, profile/command API (no registry statics).
8. **Blueprint plugin PR:** consume Graphy canvas; delete B4/B5/B6/B7/B8-generic; single in-memory document.
9. **Render Graph PR (Pulsar-Native):** port `render_bps` to the Graphy profile; shrink `model.rs`; keep `model.rs` tests.
10. **Shader plugin PR:** consume Graphy canvas; texture-preview hook; merge legacy codec; delete S1/S3/S4/S5-generic/S8.
11. **Pulsar-Native PR B:** re-pin all submodules, docs (`.agents/CRATES.md`, `RENDER_GRAPH.md`, new `GRAPHY.md`), CI; Phase 7 measurements; gate report.

### 6.6 Exact commands for later phases

```bash
# Graphy workspace (from crates/third-party/graphy once it is a workspace)
cargo test --workspace
cargo test -p graphy-blueprint          # schema + fixtures, headless
cargo bench -p graphy                    # compile-latency baseline
# headless guard
cargo tree -p blueprint_compiler -e normal | grep -E "wgpu|gpui" && echo FAIL

# Pulsar-Native root (current names)
cargo test -p blueprint_graph -p blueprint_compiler -p graphy
cargo test -p render_bps_plugin
cargo test -p pulsar_game --test random_world_blueprint
just check
just test
just clippy

# Plugin suites (separate repos; currently require the O3 fix to build)
cargo test --manifest-path plugins/vendor/blueprint_editor/Cargo.toml
cargo test --manifest-path plugins/vendor/shader_editor/Cargo.toml

# Hit-test baseline (blocked until O3)
cargo run --release --example spatial_index_stress   # in plugins/vendor/blueprint_editor

# User-run GPU (manual)
cargo run --release --example standalone_stress      # Blueprint, ~2k nodes
```
Unverified: I did not run `cargo test -p render_bps_plugin`, `blueprint_compiler`, `pulsar_game` or the shader/blueprint plugin suites (heavy builds, some need GPU/plugin checkouts); their current pass state is unknown.

## 7. Open decisions before Phase 1

- **O1.** Fate of `graphy::core::document` (`BlueprintDocument` v2): adopt as the base of the shared document, or delete it and grow the shared document from `blueprint_graph`? (D1)
- **O2.** Keep package name `graphy` as core+compiler owner (recommended) vs rename to `graphy-core`/`graphy-compiler` and migrate the ≈25 psgc/wgsl_std/plugin files in the same change.
- **O3.** How should plugin submodules be built standalone and for benchmarks: fix their stale git-rev pins (Pulsar-Native `6c61db9`, Graphy `c439240`), or officially make root-workspace builds the only supported path (then examples/benches must live somewhere in the workspace)?
- **O4.** Scope of the editor *shell* duplication (≈5k LOC per plugin): out of scope as the plan says, or an explicit later phase?
- **O5.** Whether `code_editor`'s `graphy` dependency (M12) is intentional.
- **O6.** Whether to add GPU timestamp instrumentation to the renderers as a standalone PR first (needed for any GPU baseline).
