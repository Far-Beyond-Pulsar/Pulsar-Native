# Architecture

A tour of how Pulsar-Native is put together, for someone reading the code by
hand for the first time. It stays short on purpose: each section names the
crates and files to open and points into [`.agents/`](.agents/INDEX.md), where
the subsystem documents go into depth. Those documents are written from the
source and are the place to check details; this page is the map.

## What this is

Pulsar is a game engine and level editor written in Rust. The editor binary is
`crates/core/engine` (package `pulsar_engine`). Its user interface is built on
GPUI, a GPU-accelerated UI framework vendored as a fork under `crates/ui/`. A
project the editor opens is a game project: Pulsar generates a standalone Rust
game crate for it instead of running the game inside the editor process
(`.agents/ROADMAP.md`, `crates/core/pulsar_project`, `crates/core/pulsar_package`).

Scene data lives in SceneDB, an archetype ECS in its own repository that the
workspace pins by git revision (`Cargo.toml`, `pulsar_scenedb`). Rendering is
Helio, a render-graph renderer in the `crates/renderer/helio` submodule. Almost
everything that is a kind of object (meshes, lights, sky, water, physics,
input, animation) or an editing tool is a plugin under `plugins/vendor/`, and
most plugins are separate repositories pulled in as git submodules. Clone with
`--recursive` (or run `just submodule-init`); the workspace does not load
without them.

## Layers

Every crate has a layer, set by its directory in `architecture/layers.toml`:

| Layer | Where | May depend on |
|---|---|---|
| foundation | `crates/ui`, `crates/third-party`, `crates/external`, `crates/renderer` (Helio) | foundation |
| kernel | `crates/core`, `crates/editor`, `crates/subsystems`, plus `agent_chat_core` and `agent_chat_tools` | kernel, foundation |
| tool | `crates/agent-providers`, and the editor tools `ui_git_manager`, `ui_flamegraph`, `ui_log_viewer`, `ui_fab_search` | tool, kernel, foundation |
| plugin | `plugins/vendor/*` | plugin, tool, kernel, foundation |
| host | `plugins/installed`, `tools/`, the `pulsar_engine` binary, and `pulsar_package`, `level_migrate`, `scene_inventory` | everything |

The rule that matters most: the kernel knows no object kind, subsystem or
tool. Core and editor code may not depend on a plugin or a tool, and dev
dependencies count, so a test that needs plugins belongs to a host
(`plugins/installed/tests`). Kernel code may also not name a plugin's kind in
identifiers or strings; the `[kinds]` word lists in `layers.toml` define what
counts as naming one. The checker runs as `just arch` (it is Helio's
`check_layers.py`, run over this repository) and the existing exceptions are
recorded in two lists that may only shrink: `architecture/layer-violations.txt`
and `architecture/kind-names.txt`. The kernel reaches a plugin only through
registrations, never by name. Crate-by-crate detail is in
[`.agents/CRATES.md`](.agents/CRATES.md).

## The plugin model

A plugin is reached through exactly one door, its root crate. Details are in
[`.agents/PLUGIN_SYSTEM.md`](.agents/PLUGIN_SYSTEM.md); the outline is:

- **Project plugins** (every first-party plugin today). A directory under
  `plugins/vendor/<dir>` holds a `pulsar-plugin.toml` (its id, whether it is on
  by default, the contracts it provides and requires, its root crate) and
  several crates: an `api` crate of contracts other plugins may read, a
  Helio `feature`, a `component` crate with the SceneDB components, an `editor`
  crate, and the root crate (`<kind>_plugin`).
- **The root crate is the single registration point.** It has one function,
  `build(p: &mut pulsar_plugins::PluginBuilder)`, that adds every component
  class, editor, asset kind and other contribution the plugin brings. Nothing
  else in the plugin registers anything, and no crate depends on a root except
  `plugins/installed` (generated from the descriptors) and generated games.
  `plugins/installed/tests/installed_set.rs` enforces this.
- **Enablement.** The editor links all installed roots (`installed_plugins`).
  A project turns some off in its `Pulsar.toml` `[plugins]` table;
  `pulsar_plugins::activate_project` makes that the process's set, and every
  registry skips a registration whose plugin is disabled (the plugin gate in
  `crates/core/pulsar_plugins`). A game built for a project links only its
  enabled roots.
- **Dynamic DLL plugins.** A plugin can instead be a native `cdylib` loaded at
  startup by `crates/core/plugin_manager` and never unloaded. The host and the
  library must come from the same compiler. A library that calls
  `pulsar_plugins::export_library!` hands its descriptors and registrations to
  the same installed set and catalog that linked plugins use. The SDK for
  editor plugins is `crates/core/plugin_editor_api`.

Core and editor consume plugins through registry types such as
`ClassSpec`, `ActorRegistration` and `Language`; those live in kernel crates
(`pulsar_world_registry`, `pulsar_component_lifecycle`, `pulsar_language`) so
no kernel crate has to name a plugin.

## Startup

The editor starts in `crates/core/engine/src/main.rs` (GPUI application setup is in `startup.rs`) and runs
[`.agents/ENGINE_LIFECYCLE.md`](.agents/ENGINE_LIFECYCLE.md) in this order:

1. `main` does process-level setup (TLS provider, dialog helper, thread
   registration, argument parsing).
2. It builds an `InitGraph` (`src/init/graph.rs`): a DAG of named tasks with
   declared dependencies, sorted topologically and checked for cycles. Each task
   is a file in `src/steps/`: logging, app data, settings, the Tokio runtime,
   the engine backend, the engine context, source-build detection, the global
   context, Discord presence and URI-scheme registration. The graph runs the
   tasks one after another today.
3. `EngineBackend::init` (`crates/core/engine_backend/src/lib.rs`) is the backend
   task. It owns the long-lived runtime pieces such as the GPUI application
   handle and file watchers. It holds no registry of subsystems; plugins add
   per-frame work as registrations.
4. `EngineContext` (`crates/core/engine_state/src/context.rs`) is the global
   hub, reached with `EngineContext::global()`. It holds a `StateStore`, a
   `TypeId`-keyed table of `ResourceHandle<T>`, plus per-window state.
   `engine_state::set_project_path` is also where a project's plugin set is
   activated.
5. `main` creates the GPUI application with the engine's own wgpu device
   factory and, in `App::run`, initializes the UI and themes, then `ui_core::init`
   (the editor shell, plugin manager and plugin libraries) and the window
   registry, and opens the project through the loading screen
   (`ui_loading_screen`) into `ui_core::PulsarRoot::open`.

## Scene data and rendering

One scene handle, `engine_backend::scene::SharedScene` (an
`Arc<RwLock<SceneDb>>`), is shared by the editor, the renderer, the game tick and
Play-in-Editor. Every scene object is an entity with the base components of
`pulsar_scene_model` (`StableId`, `Name`, `Transform`, `Visibility`, hierarchy),
and every component attached to an object is its own entity linked to its owner.
Component types are reflected Rust structs; `pulsar_world_registry` registers them
by class name, decodes files at the boundary, and drives their lifecycle
(`construct`, `begin_play`, `tick`, `end_play`, from `pulsar_component_lifecycle`).
JSON is only the file format. See
[`.agents/SCENEDB_MIGRATION.md`](.agents/SCENEDB_MIGRATION.md) and
[`.agents/ECS.md`](.agents/ECS.md).

Data flows one way. An edit writes the typed value straight into the SceneDB
`World`; fields marked `#[gpu]` are mirrored to GPU rows inside that write; the
renderer flushes the dirty rows once per frame and draws from them. Nothing in
the render path scans the scene. Views such as the outliner subscribe in the
other direction. `engine_backend::scene::helio_bridge` connects the mirror to
Helio, and `helio_renderer::SceneRenderer` is the one renderer construction the
editor viewport, the standalone game and Play-in-Editor all use. Components'
type-specific rendering logic belongs to their plugin and Helio feature, not to
`engine_backend`. Rendering rules are in Helio's `helio-architecture` skill
(see `AGENTS.md`), and the render-runtime service is in
[`.agents/RENDER_RUNTIMES.md`](.agents/RENDER_RUNTIMES.md).

## One path, traced: an edit, then a save

The user selects an object and types a new X position in the Properties panel,
then saves. Each hop below was read in the code.

1. **Field edit.** `crates/editor/ui_level_editor/src/ui/properties/transform_section.rs`
   builds each of the nine transform fields with `axis_binding`. Its setter closure
   reads the current transform and builds a `SceneCommand::SetTransform` that
   carries only the changed axis.
2. **Command executor.** The setter calls `execute_command`
   (`src/core/commands/executor.rs`). It captures an undo checkpoint of the
   affected objects, runs `apply_command`, and commits a labeled checkpoint only if
   the command reported a change.
3. **Write into SceneDB.** `apply_command` routes the command to `set_transform` in
   `src/core/commands/object.rs`, which validates it and calls
   `scene_edit::objects::set_transform` (`src/core/scene_edit/objects.rs`). That
   function looks the entity up by id, gets the `WorldTransform` component with
   `get_mut`, assigns the changed fields and re-lays-out generated children. The
   command then bumps the scene revision and marks the level unsaved
   (`LevelEditorState::bump_revision`, `src/state/scene.rs`).
4. **GPU mirror and frame.** On the next frame the editor renderer's frame
   path calls `SceneRenderer::upload` (`crates/core/engine_backend/src/subsystems/render/helio_renderer/scene_renderer.rs`,
   called from `renderer.rs`). `upload` runs
   `world.flush_gpu_mirror(&queue)` and closes the frame's change window, then
   `render` draws the frame through Helio. The moved object appears on screen.
5. **Save.** Choosing Save Scene in the menu
   (`src/ui/toolbar/level_editor_menus.rs`) dispatches the `SaveScene` action.
   `on_save_scene` (`src/ui/panel/handlers/playback.rs`) calls
   `ui::save::save_level`, which runs `save_blocking` (`src/ui/save.rs`) on a
   background thread.
6. **Snapshot, then write.** `save_blocking` scans the project's class registry
   with no lock held, takes a read lock on the shared scene to build
   `level_io::snapshot_level` (`src/core/scene_edit/level_io.rs`), then calls
   `level_io::write_level`, which serializes JSON and writes it with
   `engine_fs::virtual_fs::write_file_atomically`. Saves are numbered so a slow
   save never overwrites a newer one.
7. **Virtual filesystem.** `virtual_fs` routes the write to the active provider:
   local disk, a remote studio project or a peer-to-peer session
   ([`.agents/FILESYSTEM.md`](.agents/FILESYSTEM.md)). No code in the kernel calls
   `std::fs` directly; a clippy rule in the root `clippy.toml` forbids it.

## Where to go next

- Crate roles: [`.agents/CRATES.md`](.agents/CRATES.md). Each crate under
  `crates/core` has a README with what it owns and what it must not depend on.
- Project philosophy and why the engine generates a game crate:
  [`.agents/ROADMAP.md`](.agents/ROADMAP.md).
- Reflection and the type system: [`.agents/REFLECTION.md`](.agents/REFLECTION.md).
- Scripting and languages: [`.agents/SCRIPT_LANGUAGES.md`](.agents/SCRIPT_LANGUAGES.md).
- Editor tasks and threads: [`.agents/EDITOR_TASKS.md`](.agents/EDITOR_TASKS.md).
- Plans and their status: [`.agents/INDEX.md`](.agents/INDEX.md).
- Build, test and gate commands: `justfile` (`just check`, `just test`,
  `just clippy`, `just arch`).
