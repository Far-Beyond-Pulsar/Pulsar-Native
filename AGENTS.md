# Pulsar-Native

A next-generation Rust game engine where the editor is the runtime and the
runtime is the editor. For the full context, see `.agents/`.

## Quick index

| File | What |
|---|---|
| `.agents/ROADMAP.md` | Project philosophy, core insights, architecture overview |
| `.agents/CI.md` | CI workflows, cost rules, how to trigger the full cross-platform run |
| `.agents/CRATES.md` | Complete crate layout — core, editor, subsystems, agents, vendored |
| `.agents/ENGINE_LIFECYCLE.md` | InitGraph startup, EngineBackend (no subsystem registry), EngineContext |
| `.agents/PLUGIN_SYSTEM.md` | Project plugins (descriptor, root crate, gate, catalog) and dynamic DLL plugins (permanent library, SDK, loading) |
| `.agents/PLUGIN_API_REFACTOR_PLAN.md` | The plugin API plan (#1608): the phase 0 audit (registration matrix, raw `inventory` paths, plugin-to-plugin edges), its review, the Q1-Q4 decisions, phases, as built |
| `.agents/COMPONENT_LIFECYCLE_PLAN.md` | The component lifecycle plan: audit of SceneDB `Actor`/`ActorRegistry`, `ComponentTickRegistration` and the script driver, the target (capability registry, `construct`, session scope, physics, Blueprint class instances), decisions, phases P1-P6, risks |
| `.agents/FILESYSTEM.md` | `engine_fs::virtual_fs` — local, remote, P2P providers |
| `.agents/TRAITS.md` | Trait system: ids, `TraitDb`/index, engine role traits, conformance, trait event nodes, plugin API, level roles at runtime |
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
| `.agents/MEMORY.md` | How memory is held and bounded (`Residency`, texture size cap), settings, and the Mission Control Memory Explorer |
| `.agents/LOD.md` | Authored mesh LOD as a camera-dependent asset choice, separate from renderer-internal geometric LOD, and its residency boundary |
| `.agents/RENDER_GRAPH.md` | Render pipelines: `IntoGraph`, `.rendergraph` assets, the shipped pipelines (`pulsar.default_pipelines` plugin, old-id migration), pin shapes, the `render_bps` plugin |
| `.agents/RENDER_RUNTIMES.md` | Engine-owned Helio renderers: `render_runtimes` service (one scheduler, one device, states, update modes, outputs, host-driven game runtimes), the Tasks window's Renderers view. Read before creating, drawing or presenting a renderer anywhere |
| `.agents/GRAPH_CANVAS.md` | Shared node-graph canvas in Graphy: `graphy-render` layout/scene/hit-test/draw data, `graphy-render-wgpu` renderer and visual system, `graphy-gpui` interaction engine and profile API |
| `.agents/CODE_EDITOR_AI.md` | Code editor AI: shared editor LLM, Mockaco inline blocks and hunk review, the inline prompt (Ctrl+I), chat-panel buffer tools |
| `.agents/INSPECTOR.md` | Properties panels: one virtualized list, search/collapse, `VirtualRows` for huge values (arrays, lists, layer stacks) |
| `.agents/EDITOR_TASKS.md` | Editor tasks and threads: every async/background job is a task, every thread is registered (services with heartbeats, rayon/tokio pools engine-owned), each with span-scoped logs and stall diagnosis; the Tasks window has Tasks/Threads/Services views; a raw-thread ratchet. Every async/background editor job is a task with its own span-scoped log, stall diagnosis and a Tasks-window audit view; tab creation and Helio init are enforced. Read before adding any spawn, background job or editor/renderer init |
| `.agents/KEYBINDINGS.md` | Keyboard Shortcuts window, `.pulsar/keybindings.json` overrides, how unbinding reaches the keymap |
| `.agents/ANIMATION.md` | Animation assets (`.animseq` v5, `.blendspace`, `.montage`, `.animgraph` on Graphy, `.animretarget`, `.ikrig`, `.ikretarget`, `.posedb`): schema/ownership table, migrations, node catalogue, mesh-owned skeleton authority, dependency index and validators, runtime, notify/Montage event delivery, root motion, physics bone ownership, hot reload |

## Skills (`.claude/commands/`)

| Skill | When |
|---|---|
| `/add-plugin` | create a project plugin: descriptor, contracts, root crate and `build`, tests, pins |
| `/add-component` | create a world component: class, `ClassSpec`, `Actor`/`WorldActor` lifecycle, session scope, script methods and events |
| `/add-blueprint-node`, `/blueprint-system` | add a `pulsar_std` node; how a Blueprint compiles and runs |
| `/add-editor-panel`, `/gpui-patterns` | add a dockable editor panel; GPUI idioms |
| `/bump-deps` | move a submodule pin, a plugin's Pulsar rev or the SceneDB rev (three manifests) |
| `/debug-compile-errors`, `/repo-map` | duplicate-crate and Blueprint compile errors; where things live |

## Project data (`.pulsar/`)

| Path | What |
|---|---|
| `.pulsar/import_db.json` | Linked source imports: source, hash, native asset, importer, and for a rigged model the other files it made (`asset_import`) |
| `.pulsar/cache/posedb/` | Derived Pose Search indexes, keyed by settings and source-content hash (never written beside the source); gitignored |
| `.pulsar/keybindings.json` | The project's keybinding overrides over the defaults (`ui_keybindings`, see `.agents/KEYBINDINGS.md`) |
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
just check-submodule-pins   every submodule pinned to a commit on its upstream main
```

Submodule changes merge dependency-first: merge the dependency PR into its
main, re-pin the submodule to that main commit, then merge the Pulsar PR (see
"Submodule pins" in `.agents/CRATES.md`).

See `.agents/` for detailed docs on each subsystem.

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
