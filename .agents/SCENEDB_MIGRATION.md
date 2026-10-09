# SceneDB scene data

SceneDB owns all scene state. This page describes the architecture as built
by the corrective plan (Pulsar-Native#1035; one PR per phase, #1036 to #1084).
`crates/core/scene_inventory/ledger.toml` has one closed row per component class, GPU schema,
buffer, render pass and lifecycle call site, and `cargo test -p scene_inventory`
checks it against the linked registries and the source tree.

## Data flow

Data flows one way:

1. An edit (properties panel, gizmo, command, script, plugin, asset drop,
   class placement) writes the typed value straight into the `World`:
   `insert`, `get_mut`, `insert_dyn`, or a reflected setter through
   `get_dyn_mut`. There is no intermediate queue, JSON copy or "refresh"
   call.
2. The `World` mirrors `#[gpu]` fields to GPU rows inside that write.
   `World::flush_gpu_mirror` uploads the dirty rows once per frame.
3. Systems that need data read the `World` or the GPU mirror. Renderers draw
   from the mirror; Helio's scene and environment joins derive the rows its
   passes consume, on the GPU.

Nothing in that direction subscribes. Subscriptions run the other way, for
views (see [Observers](#observers)).

## Ownership

- `engine_backend::scene::SharedScene` (`Arc<RwLock<pulsar_scenedb::SceneDb>>`)
  is the one scene handle. The editor, the renderer, the game tick and
  play-in-editor share it. There is no second scene store, snapshot or
  metadata database.
- Every scene object is a SceneDB entity carrying typed components:
  `StableId`, `Name`, `Transform`, `Visibility`, and the hierarchy
  (`Parent`, `SiblingIndex`).
- Every component attached to an object is its own **component-instance
  entity** (`pulsar_scene_model::attachments`): the typed value, a
  `ComponentOwner` (owner object and enabled flag; GPU-mirrored so passes
  join a component row to its owner on the GPU) and a `ComponentMeta`
  (stable `ComponentInstanceId`, class name, presentation parent, class-slot
  provenance). The object's `ComponentAttachments` lists them in
  presentation order. Several instances of one class are several entities.
- A class the build does not know, or a record that fails to decode, is kept
  as an `UnresolvedComponent` that holds the payload verbatim and is never a
  live component.
- `RenderProps` holds an object's free-form file `props` only. It holds no
  component list.

## Components and registration

- `#[register_world_component]` on an inherent `impl Type {}` registers a
  component class with `pulsar_world_registry` (class name = type name):
  default factory, boundary decoder, clone, erased insert/remove.
- `#[derive(SceneStore)]` with `#[gpu]` fields generates the GPU mirror.
  `GpuHeavy<T>` keeps heavy GPU data out of the component column: the column
  holds only the reference.
- A component with no runtime consumer yet is declared with
  `declare_unfinished_component!` (reason and tracking issue) and is
  reported when attached, instead of looking supported.
- Native component lifecycles (`begin_play`, `tick`, events, `end_play`)
  run from `ComponentTickRegistration` on the live typed value.

## JSON

JSON exists only at boundaries: level files, the offline migration tool, the
record API (`pulsar_world_registry::instances`), reflection save codecs and
asset files. A level's records are migrated (`pulsar_class::records`) and
decoded once, at load, into component-instance entities. History, undo,
class instantiation, scripts and the editor clone and write typed values.

## Observers

- **Change-journal cursors** (`World::open_change_cursor` / `read_changes`):
  for incremental work. Each reader owns its cursor and sees every change
  regardless of other readers; `ChangeRead::Overflowed` (the journal evicted
  unread entries, or the cursor came from another `World`) tells the reader
  to rescan once. Users: renderer-side cursor-gated rebuilds, script
  `ComponentRefWatch` (`pulsar_world_registry::ComponentWatch`), component
  lifecycle removals.
- **Object subscriptions** (`World::subscribe_object`): for views. A view
  that displays an object follows writes made elsewhere (gizmo, script,
  undo) without polling. The callback runs inside the write with the
  component's full new value. `pulsar_world_registry::ObjectFeed` clones it,
  queues an `ObjectUpdate` and wakes the view; the properties panel follows
  the selected object this way. Renderers and other bulk readers never
  subscribe.
- The world's change tracker (`World::revision`) paces idle frames; it is
  closed once per frame by `engine_backend::scene::end_change_window`.

## Renderer

- The renderer holds the `SharedScene` to flush the GPU mirror, read the
  revision for frame pacing, and serve picking and selection. It does not
  scan the scene to discover or project render components.
- `engine_backend::scene::helio_bridge` attaches the mirror and builds the
  scene and environment joins. `ensure_gpu_mirror` attaches a mirror late;
  SceneDB replays existing rows on attach.
- Helio has no renderer-owned scene registry; the legacy `Scene`/`SceneActor`
  API is gone ([HELIO_SCENE_API_MIGRATION.md](HELIO_SCENE_API_MIGRATION.md)).
- The standalone game and the Play-in-Editor viewport build their renderer
  with `pulsar_game::game_renderer`: the same joins and graph as the editor
  viewport, over the same `SharedScene`.

## Assets

A mesh asset loads synchronously inside the write that names it (the
boundary decoder, or the `mesh_asset` property write), so there is no
in-flight load to cancel or complete stale. Components of one asset share one
GPU geometry allocation. A re-import publishes `AssetUpdated(Mesh)`; the level
editor reloads every static mesh naming that file through the same write.

## Architecture checks

`crates/core/scene_inventory/tests/architecture.rs` fails when:

- a removed mechanism gets a production call site again: the shared change
  queue and its drains, GPU refresh and render arming hooks,
  `PendingWorldWrites`, CPU projection into pass rows, `RenderProps`
  component sync, runtime-behavior dispatch, force-resync;
- a JSON record is decoded outside the listed boundary files;
- renderer code subscribes to an object;
- a renderer file gains a `World::query` call that is not listed with its
  reason (picking, bake input, cursor-gated rebuilds, the voxel path).

## Working rule

Do not introduce another scene owner, snapshot cache, lock wrapper, write
queue or async facade. If an API cannot be built without one, stop and
record the specific boundary in the plan.
