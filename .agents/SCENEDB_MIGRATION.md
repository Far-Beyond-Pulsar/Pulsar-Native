# SceneDB Migration

## Objective

Make SceneDB the sole owner of scene state. The editor must not maintain a
second scene representation, and the renderer must not share or lock the CPU
scene world.

## Non-negotiable architecture

- `pulsar_scenedb::SceneDb` owns the authoritative `World` and its lifecycle.
- Every scene object is a SceneDB entity with typed components.
- Scene identity, hierarchy, transforms, visibility, render data, and runtime
  component data live in SceneDB components.
- `#[derive(SceneStore)]` and `#[gpu]` are the GPU integration path. Normal
  `World::insert` and `World::get_mut` calls update CPU storage and queue GPU
  mirror changes; `SceneDb::step`/`flush_gpu_mirror` performs the coalesced
  upload.
- SceneDB's phase machine supplies the single-writer/shared-reader discipline.
- The renderer consumes SceneDB-owned GPU buffers and GPU-side liveness/
  generation state. It does not read or mutate the CPU `World` every frame.
- The level editor owns only editor/UI state: tools, panels, camera controls,
  expansion state, overlays, build state, and play controls.
- No `Arc<RwLock<WorldSceneStore>>`, copied scene snapshots, renderer-owned
  scene caches, or parallel metadata scene database may remain in production.

## Transitional code to remove

- `engine_backend::scene::WorldSceneStore` as an object/hierarchy authority.
- `ui_level_editor::SceneDatabase` as scene storage.
- `SceneMetadataDb`/JSON component storage for data that belongs in typed
  SceneDB components.
- Renderer `scene_store` locks and per-frame CPU scene scans.
- `HelioRenderer::sync_scene`/`sync_scene_delta` as a second scene-to-render
  synchronization system.
- Manual light/mesh transform rebuilds where SceneDB GPU fields already carry
  the data.
- PIE raw pointers and long-lived write guards to scene state.

## Migration sequence

1. Inventory every `WorldSceneStore` and `SceneDatabase` caller.
2. Define/register canonical SceneDB components for scene identity,
   hierarchy, editor-visible object properties, and runtime component data.
3. Move lifecycle construction/loading/saving to an owning engine/lifecycle
   object containing `SceneDb`.
4. Convert editor scene commands and hierarchy/property queries to operate on
   the owning SceneDB world.
5. Remove the outer scene lock from renderer construction and make renderer
   consume SceneDB GPU resources/change results only.
6. Remove transitional wrappers, indexes, dirty flags, and JSON mirrors after
   all callers migrate.
7. Add adversarial tests in each affected crate for identity, hierarchy,
   despawn/reuse, component mutation, GPU dirty propagation, and concurrent
   editor-only state access.

## Current findings

- The editor lifecycle now constructs `SceneDatabase` first; the renderer
  receives its shared store handle afterward. The former caller-supplied store
  constructor boundary (`LevelEditorState::new_with_scene_db` /
  `SceneDatabase::with_shared_store`) has been removed, so scene construction
  has one concrete owner while the existing compatibility API remains in place
  for scene operations.

- Gizmo mode/highlight state is now editor/renderer mailbox state rather than
  `WorldSceneStore` state. This removes the mailbox's scene-lock dependency;
  the renderer still locks the shared store for object synchronization because
  stable-id/hierarchy/JSON compatibility data has not yet been replaced by
  typed SceneDB components and GPU change results.

- Remaining renderer boundary: `HelioRenderer` still owns an
  `Arc<RwLock<WorldSceneStore>>` because its current object sync path needs
  stable-id lookup, hierarchy traversal, component JSON projection, dirty
  draining, and the `SceneDb::step` mutable lifecycle call. Removing that
  lock in the next slice requires first routing those operations through
  SceneDB-owned typed components/change results; replacing it with another
  wrapper or snapshot would violate the migration constraints.

- The renderer no longer consumes the unused `SceneDbDelta.revision` field or
  reads `WorldSceneStore::dirty_gen()` while draining a delta. The remaining
  dirty vectors still carry the compatibility flags needed to dispatch legacy
  object/component JSON updates. Native `ChangeTracker` is not yet a drop-in
  replacement: it has no non-destructive pending query in the pinned README/API,
  and its post-despawn entity signal no longer resolves stable IDs.

- The renderer's remaining `step()` calls are intentionally after component
  dispatch because that dispatch can queue GPU-mirror refreshes that must be
  flushed in the same pass. The play-mode lifecycle already uses
  `scene::step_scene_for_render`; moving the editor renderer's call to a
  generic engine tick would require that tick to own the editor's pending
  component dispatch/refresh boundary as well. Moving `step()` earlier would
  defer uploads or change ordering, so no safe relocation was made in this
  slice.

- Audited the next duplicated-field candidates. `name`, `visibility`, and
  `object_type` are already stored canonically as `WorldSceneStore`'s
  `Name`, `Visibility`, and `ObjectType` components; `SceneDatabase` setters
  and reads route to those components. `SceneObjectData` is only a serialized
  UI/API value adapter, so removing its fields would be a broad public
  serde/API break rather than removing an authority. `SceneMetadataDb` still
  owns JSON component instances for classes without typed World components;
  migrating that requires per-class typed registrations and cannot be safely
  collapsed in this slice. Added a regression test proving the canonical
  World component round trip.

- Full authority removal is currently blocked at the compatibility API seam,
  not by an unexamined field: `SceneDatabase::get_object` and
  `get_all_objects` must still return the public serde-compatible
  `SceneObjectData` value, while renderer dispatch still consumes serialized
  component instances for classes that have no typed World registration.
  Removing those adapters or `SceneMetadataDb` in one pass would either break
  the editor/plugin API or drop untyped component behavior. The next valid
  migration must add typed World registrations and migrate one component class
  at a time before the metadata projection and renderer lock can be removed.

- `Transform` already uses `#[derive(SceneStore)]` with packed `#[gpu]`
  fields.
- The renderer currently still receives `Arc<RwLock<WorldSceneStore>>` and
  performs CPU scans/rebuilds despite that GPU path.
- `WorldSceneStore` adds stable-id maps, hierarchy maps, dirty flags, revision
  counters, and a public `world_mut` escape hatch around SceneDB.
- `SceneDatabase` still owns a shared store and a separate metadata database.
- `pulsar_scenedb` README says GPU fields are automatically mirrored and that
  CPU storage uses single-writer/shared-reader phase discipline; it does not
  require a copied scene snapshot.

## Working rule

Do not introduce another scene owner, snapshot cache, lock wrapper, or async
facade. If an API cannot be migrated without one, stop and document the
specific boundary instead of inventing a parallel representation.
