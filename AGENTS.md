# Pulsar-Native

A next-generation Rust game engine where the editor is the runtime and the
runtime is the editor. For the full context, see `.agents/`.

## Quick index

| File | What |
|---|---|
| `ARCHITECTURE.md` | Human-oriented tour: layers, plugin model, startup, scene data, one traced editor action |
| `.agents/ROADMAP.md` | Project philosophy, core insights, architecture overview |
| `.agents/CI.md` | CI workflows, cost rules, how to trigger the full cross-platform run |
| `.agents/BUILD_LESSONS.md` | Why these builds went wrong and what not to do: per-image statics, feature unification and the workspace hack, build-script environment leaks, floating git dependencies, hash-named libraries, per-platform linking; the diagnostics that worked. Read before changing linking, features or a CI build |
| `.agents/CRATES.md` | Complete crate layout — core, editor, subsystems, agents, vendored |
| `.agents/ENGINE_LIFECYCLE.md` | InitGraph startup, EngineBackend (no subsystem registry), EngineContext |
| `.agents/PLUGIN_SYSTEM.md` | Project plugins (descriptor, root crate, gate, catalog) and dynamic DLL plugins (permanent library, SDK, loading), and linked mode (`--features link-plugins`: every plugin in the executable) |
| `.agents/SCRIPT_LANGUAGES.md` | Script languages as built: the `Language` registration, the `ScriptLanguage` contract, the module store and one-language-per-class rule, class markers, the server spec and settings, diagnostics sources, `ModuleBuilder` (what a text language emits modules through) with a worked example, what the VM can and cannot express, plugin layout, testing, the layer gate. Read before adding a language |
| `.agents/PLUGIN_API_REFACTOR_PLAN.md` | The plugin API plan (#1608): the phase 0 audit (registration matrix, raw `inventory` paths, plugin-to-plugin edges), its review, the Q1-Q4 decisions, phases, as built |
| `.agents/COMPONENT_LIFECYCLE_PLAN.md` | The component lifecycle plan: audit of SceneDB `Actor`/`ActorRegistry`, `ComponentTickRegistration` and the script driver, the target (capability registry, `construct`, session scope, physics, Blueprint class instances), decisions, phases P1-P6, risks |
| `.agents/LANGUAGE_PLUGINS_PLAN.md` | The language plugins plan: one `Language` registration (metadata, script compiler, language server), `LanguageServers` + `DiagnosticsStore`, rust-analyzer and typescript-language-server as plugins; audit of `pulsar_lsp`, its consumers and the `class.ts`/`graph_save.json` hardcoding, decisions, phases P1-P6, risks, issues log |
| `.agents/COMPONENT_EVENT_SOURCES_PLAN.md` | The component event sources plan: asset kinds (and reflected properties) declare the Blueprint events a component offers, generic for any plugin; parametric event families, the `On Input Action` node, cache/invalidation and performance budgets, migration of animation and input, phases P0-P6, unverified list |
| `.agents/LEVEL_LOADING.md` | Level loading audit: the prepare (worker) / apply (frame thread) split, which paths block, what remains. Read before adding any level, sublevel or asset load |
| `.agents/FILESYSTEM.md` | `engine_fs::virtual_fs` — local, remote, P2P providers |
| `.agents/TRAITS.md` | Trait system: ids, `TraitDb`/index, engine role traits, conformance, trait event nodes, plugin API, level roles at runtime |
| `.agents/SETTINGS.md` | Settings: the editor, project and plugin namespaces and who may register each, the Project Settings and Editor Settings windows (three-level tree, measured pages, search), appearance applied live, reading a setting |
| `.agents/PROBLEMS.md` | The Problems panel (tabs, grouped one-line rows, dismiss) and findings: the `pulsar_findings` model and store, who publishes what, how a plugin or render pass reports a finding |
| `.agents/REFLECTION.md` | Type system — `Reflectable`, `RuntimeTypeInfo`, `EngineClass` |
| `.agents/ECS.md` | SceneDB's archetype ECS — `World`, queries, journals, `Schedule`, actors |
| `.agents/SCENEDB_MIGRATION.md` | Scene data as built: SceneDB ownership, component instances, data flow, observers, renderer, architecture checks |
| `.agents/FRAMES.md` | Parent-relative transforms, GPU hierarchy resolve (as built), and the planned two-tier frame design for sublevels/portals |
| `.agents/SKY.md` | Sky texture layer — equirect panorama under the procedural sky, controls, data flow, CC0 presets fetched by build.rs |
| `.agents/LENS.md` | Camera lens — graph placement, components, element kinds, presets, prescription bank, masks, overscan, user WGSL hook |
| `.agents/LAYOUT.md` | Layout persistence — `.pulsar/layout.json` v2 slots, resolve/validate/commit restore, restorers + dormant tabs, zoom, chrome, pop-outs |
| `.agents/FILE_MANAGER.md` | `ui_file_manager` — flat crate layout, modules, conventions |
| `.agents/THEMING.md` | Theme JSON schema, syntax highlighting, window backgrounds |
| `.agents/UI_CRATES.md` | UI crate conventions — flat layout, `components/`/`handlers`/`utils` pattern |
| `.agents/PANEL_CANVAS.md` | `ui::panel_canvas`: Panel Canvas slots, RnD (resize and drag) panels, drag/resize handles with window-level tracking; the viewport overlays use it; gallery example |
| `.agents/MEMORY.md` | How memory is held and bounded (`Residency`, texture size cap), settings, and the Mission Control Memory Explorer |
| `.agents/LOD.md` | Authored mesh LOD as a camera-dependent asset choice, separate from renderer-internal geometric LOD, and its residency boundary |
| `.agents/RENDER_GRAPH.md` | Render pipelines: `IntoGraph`, `.rendergraph` assets, the shipped pipelines (`pulsar.default_pipelines` plugin, old-id migration), pin shapes, the `render_bps` plugin |
| `.agents/RENDER_RUNTIMES.md` | Engine-owned Helio renderers: `render_runtimes` service (one scheduler, one device, states, update modes, outputs, host-driven game runtimes), the Tasks window's Renderers view. Read before creating, drawing or presenting a renderer anywhere |
| `.agents/GRAPH_CANVAS.md` | Shared node-graph canvas in Graphy: `graphy-render` layout/scene/hit-test/draw data, `graphy-render-wgpu` renderer and visual system, `graphy-gpui` interaction engine and profile API |
| `.agents/CODE_EDITOR_AI.md` | Code editor AI: shared editor LLM, Mockaco inline blocks and hunk review, the inline prompt (Ctrl+I), chat-panel buffer tools |
| `.agents/INSPECTOR.md` | Properties panels: one virtualized list, search/collapse, `VirtualRows` for huge values (arrays, lists, layer stacks) |
| `.agents/EDITOR_TASKS.md` | Editor tasks and threads: every async/background job is a task, every thread is registered (services with heartbeats, rayon/tokio pools engine-owned), each with span-scoped logs and stall diagnosis; the Tasks window has Tasks/Threads/Services views; a raw-thread ratchet. Every async/background editor job is a task with its own span-scoped log, stall diagnosis and a Tasks-window audit view; tab creation and Helio init are enforced. Read before adding any spawn, background job or editor/renderer init |
| `.agents/KEYBINDINGS.md` | Keyboard Shortcuts window, `.pulsar/keybindings.json` overrides, how unbinding reaches the keymap |
| `.agents/MOCAP.md` | Motion capture plugin (`pulsar.mocap`): the `.mocap` take format, capture sources (camera, VMC, synthetic), recorder, the editor workspace and camera viewport, phases |
| `.agents/MCP.md` | The engine's MCP server: Streamable HTTP on loopback with a bearer token and Host/Origin checks, how a crate or plugin registers a tool (`pulsar_mcp::McpToolRegistration`), the tool catalogue (build, logs, tasks, threads, problems, plugins, open file, and the editor's existing AI tools), how to connect a client |
| `.agents/GAMEPLAY.md` | Build & Run as built (regenerate, pack, build, run, findings, game output as a task log), how a level's gameplay roles become actors and what their scripts hear, the default player character (capsule, camera rig, look, move), what was broken and what is known to be missing |
| `.agents/VOXEL.md` | Voxel worlds: a pointer to the plugin's own docs, what a level's voxel load-in costs in the editor (measured) and how to measure it again from `engine.log` |
| `.agents/ANIMATION.md` | Animation assets (`.animseq` v5, `.blendspace`, `.montage`, `.animgraph` on Graphy, `.animretarget`, `.ikrig`, `.ikretarget`, `.posedb`): schema/ownership table, migrations, node catalogue, mesh-owned skeleton authority, dependency index and validators, runtime, notify/Montage event delivery, root motion, physics bone ownership, hot reload |

## Skills (`.claude/commands/`)

| Skill | When |
|---|---|
| `/add-plugin` | create a project plugin: descriptor, contracts, root crate and `build`, tests, pins |
| `/add-language` | add a scripting language (and its server): `Language`, front end through `ModuleBuilder`, `ScriptLanguage`, tests, checklist |
| `/add-component` | create a world component: class, `ClassSpec`, `Actor`/`WorldActor` lifecycle, session scope, script methods and events |
| `/add-blueprint-node`, `/blueprint-system` | add a `pulsar_std` node; how a Blueprint compiles and runs |
| `/add-editor-panel`, `/gpui-patterns` | add a dockable editor panel; GPUI idioms |
| `/bump-deps` | move a submodule pin, the SceneDB rev (three manifests) or the Pulsar-Profiling rev |
| `/debug-compile-errors`, `/repo-map` | duplicate-crate and Blueprint compile errors; where things live |

## Project data (`.pulsar/`)

| Path | What |
|---|---|
| `.pulsar/import_db.json` | Linked source imports: source, hash, native asset, importer, and for a rigged model the other files it made (`asset_import`) |
| `.pulsar/cache/posedb/` | Derived Pose Search indexes, keyed by settings and source-content hash (never written beside the source); gitignored |
| `.pulsar/keybindings.json` | The project's keybinding overrides over the defaults (`ui_keybindings`, see `.agents/KEYBINDINGS.md`) |
| `.pulsar/dismissed_findings.txt` | Findings the project dismissed in the Problems panel, one item id per line (`ui_problems::utils::dismissed`, see `.agents/PROBLEMS.md`) |
| `.pulsar/layout.json` | Per-user editor layout: dock tree, zoom, drawers, window geometry, pop-outs, one slot per project window (`ui_core::app::layout`, see `.agents/LAYOUT.md`); gitignored |
| `.pulsar/import_options.json` | Legacy import options; linked entries migrate into `.pulsar/import_db.json`, while convert-in-place entries remain a fallback |
| `.pulsar/trash/` | Sources replaced by a convert-in-place import, recoverable |

## Workspace

```toml
default-members = ["crates/core/engine"]
members = ["crates/core/*", "crates/editor/*", "crates/subsystems/*", "crates/agent-providers/*",
           "plugins/vendor/<kind>/{api,component,feature,editor,examples,...}", ...]
```

The plugins under `plugins/vendor/` (every object kind, physics, animation,
input, UI, Blueprints, ...) are their own repositories (`Plugin_<Kind>`),
git submodules whose crates are members of this workspace. Each plugin's
root crate (the package at its directory, `<kind>_plugin`) is the one place
it registers what it contributes; hosts reach a plugin only through its root
(`.agents/PLUGIN_SYSTEM.md`); Helio is the
`crates/renderer/helio` submodule. Clone with `--recursive` (or run
`just submodule-init`): cargo cannot load the workspace without them. The
table of plugins and their repositories is in `.agents/CRATES.md`
("plugins/vendor/ — Plugins").

Core and editor know no kind, subsystem or tool: the layer gate (`just arch`,
`architecture/layers.toml`) fails when a kernel crate (`crates/core`,
`crates/editor`, `crates/subsystems`) depends on a plugin or a tool, dev
dependencies included, or names a plugin's kind beyond the shrink-only
`architecture/kind-names.txt`. A test that needs plugins belongs to
`plugins/installed/tests` (`.agents/CRATES.md`, "The layers").

## Key commands

```
just check        cargo check
just build        cargo build -p pulsar_engine
just test         cargo test --workspace
just clippy       cargo clippy --workspace -- -D warnings
just arch         the layer gate: core and editor depend on no plugin or tool, name no kind
just submodule-init   git submodule update --init --recursive
cargo run -p plugin_builder   build every plugin except render_bps as its own DLL (its root crate, feature `dll`) and install it
just build-linked   build the editor (release) with EVERY installed plugin linked into the executable (`--features link-plugins` on `pulsar_engine`); no plugin DLLs
just run-linked --project-path <dir>   build and run that editor from the repository root (`.agents/PLUGIN_SYSTEM.md`, "Linked mode")
just check-submodule-pins   every submodule pinned to a commit on its upstream main
```

Submodule changes merge dependency-first: merge the dependency PR into its
main, re-pin the submodule to that main commit, then merge the Pulsar PR (see
"Submodule pins" in `.agents/CRATES.md`).

See `.agents/` for detailed docs on each subsystem.

## Disk and target directories

A debug build of this workspace is tens of GB per distinct build shape (measured: one test binary's closure is
27 GB; a long-lived target directory reached 450 GB in `debug/deps` alone, 90% of it older copies of crates that
the newest build no longer uses). Cargo never removes those. Rules, with the numbers in
`.agents/BUILD_LESSONS.md` section 10:

- **One shared target directory, never one per task.** Agents and worktrees set the same `CARGO_TARGET_DIR`
  (for example `D:\cargo-target\pulsar`; `debug` and `release` live side by side in it). Do not create `tb_*`,
  `target-<topic>` or per-worktree `target` directories for a run, a measurement or a worktree: each is a cold
  copy of the whole graph. If two builds must run at once, reuse a fixed pool (`...\pulsar-1`, `...\pulsar-2`),
  named by slot, not by task. Cargo's file lock makes concurrent builds in one directory wait; that is cheaper
  than a second 30 GB copy.
- **Do not vary what hashes the build.** A different `-p` set, `--features`, `RUSTFLAGS`, `CARGO_PROFILE_*` or
  `--config profile...` gives the shared crates a new hash and a second full copy (`.agents/BUILD_LESSONS.md`
  section 2). Use the repository's profiles (`Cargo.toml`) and the recipes in `justfile`; for an experiment on
  debug info, use the environment override once on a scratch directory and delete it.
- **Prune.** `just disk` reports where a target directory's space goes; `just clean-stale` (dry run) lists and
  `just clean-stale --apply` removes older copies of crates, old incremental data, stale `plugin_compat` copies
  and idle Build & Run game targets (`scripts/target-disk.ps1`; `-ScanDir D:/ -ScratchPattern 'tb*'` also lists
  idle scratch target directories). Run it when a task ends. It never touches anything modified in the last
  30 minutes, so it is safe beside a running build.
- Build & Run builds a game in `<project>/target` (one per project, reused, not per run); the dialog helper and
  tests use stable directories (`CARGO_HOME/pulsar-dialog/target-<rev>`, `target/tmp`). Add new tools the same
  way: a directory keyed by what it builds, never by the time of the run.

## Required for rendering work

Any change that touches Helio (`crates/renderer/helio`) or Pulsar's rendering
side must load the **`helio-architecture`** skill first:
[`crates/renderer/helio/.agents/skills/helio-architecture/SKILL.md`](crates/renderer/helio/.agents/skills/helio-architecture/SKILL.md).
Rendering work includes the plugins under `plugins/vendor/*`, SceneDB
components that publish GPU rows, features and passes, WGSL, render graphs and
presets, scene derivations, and object-kind editor tools. Bug fixes, perf
work, tests and small changes count too. Anything you brief must load it as
well, and end with its report section (Design, AAA comparison, Deleted,
Verified). The skill lives in Helio and is the single source; don't copy its
rules here.
