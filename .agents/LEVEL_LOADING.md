# Level loading: what blocks, and the rule

**Rule:** a thread that draws or answers input (the UI thread, a render loop,
a winit event loop) never reads a file, scans a directory, parses a level or
waits for an asset. Slow work runs on a worker and hands over plain data
(`PreparedLevelFile`); the frame thread applies it in memory.

## The two halves

| Half | Does | Where it may run |
|---|---|---|
| `RuntimeLevel::prepare` / `prepare_for_project` | read, JSON parse, class-migration, class-registry scan | a worker only |
| `RuntimeLevel::apply_prepared` / `apply_prepared_under` | spawn objects, attach components, expand classes (memory only) | a frame thread, under the world write lock |

`apply_prepared` never waits for assets (the renderer's per-frame
`pump_assets` binds them) and requests sublevels from workers
(`sublevel_display::sync_level_instances_deferred`).

The blocking forms remain, named and documented as blocking, for headless
runs, tools and tests: `RuntimeLevel::load_into[_with_classes]`, `load`,
`from_scene_file*`, `sublevel_display::sync_level_instances`,
`scene::resolve_level_assets` (waits up to `LEVEL_ASSET_WAIT`, 120 s).

## Audit

| Path | Before | Now |
|---|---|---|
| Editor open / default level (`ui_level_editor` `lifecycle.rs`) | Already async: parse on a worker, per-object write batches, asset wait as an async timer with the lock released | unchanged |
| Sublevel displays from the render loop (`helio_bridge::sync`) | **Blocking.** Read file + class scan + parse + hydrate under the world write lock, on the frame thread | File prepared by a worker (`sublevel-prepare`); the frame applies it when ready. `needs_level_instance_sync` is false while a file is pending |
| Sublevel displays after an edit (`commands/executor.rs`) and undo restore (`state/scene.rs`) | **Blocking** on the UI thread (same work) | Deferred sync |
| Editor hydration finish (`level_io::finish_prepared_hydration`) | Read sublevel files under the write lock on the loader worker | Deferred sync |
| Windowed game startup (`pulsar_game::windowed_app`) | **Blocking** the winit event loop: class scan, parse, hydrate, up to 120 s asset wait | Worker prepares level and roles (`level-prepare`); `LevelHydration::step` spends 4 ms per frame applying it, objects appear as they spawn; the tick thread starts when it is complete |
| Render-graph preview (`render_bps` `preview.rs`) | **Blocking** UI thread: load plus 120 s asset wait | `prepare` + `apply_prepared`, no asset wait |
| Hierarchy refresh for a sublevel that lands in a frame | n/a (was synchronous) | `sublevel_display::applied_generation` polled by the level editor's 50 ms loop bumps the scene revision |

## Asset streaming (SceneDB `assets.rs`)

Assets stream in through `World::pump_assets_within(AssetBudget)`, never one
unbounded call, on every frame thread:

| Piece | Behaviour |
|---|---|
| Budget | `scene::pump_assets` applies and binds at most `streaming.asset_bind_max_per_frame` (8) assets and stops after `streaming.asset_bind_budget_ms` (2 ms); 0 lifts a limit. The rest waits in the world; `AssetPumpReport::pending` counts it (loads waiting, in flight, finished but unapplied, upgrades, unbound). One asset's dependents bind together, so a texture shared by thousands of entities is one indivisible step. Headless and test callers keep `pump_assets()` (unlimited) and `resolve_assets_blocking` (waits for every tier). |
| Priority | `new_scene` installs `asset_priority`: an asset ranks by the distance from `World::asset_focus` (the render camera's eye, set by `SceneRenderer::upload` through `scene::set_asset_focus`) to the nearest entity referencing it, using its `Transform` position (a parent-relative transform ranks by its local position). At most `2 x loader threads` loads run at once, nearest first; scores are rescored every 30 pumps while requests wait, and finished loads are applied nearest first. |
| Quality tiers | A kind registered with `register_tiered_loader` loads coarse first: each tier is bound as it arrives (`AssetView::tier`/`complete`, version bumps) and the next tier is requested only after every first tier has been started and only while the asset is referenced. Textures: tier 0 is cooked at `streaming.coarse_texture_size` (128; 0 = off), tier 1 at `max_texture_size`; an image within the coarse size is complete at tier 0. The GPU copy is re-uploaded on the version change, within the existing residency and `texture_vram_budget_mb` rules. |

What the profiler showed on a real level (Bistro, 560 textures, `GAMEPLAY.md` "Streaming a level in"), and what changed:

- **Backpressure.** Loads in flight count what has loaded and is not bound yet (`start_pending_loads`): loaders used to outrun the frame, and the residency LRU evicted what they cooked before the frame bound it (the bind then re-read it from the derived cache on the frame thread).
- **Pinned cooks.** A graph material's textures are cooked on the loader thread for their real use (colour or data) and pinned in the `LoadedTexture` until the upload (`pinned_for` / `unpin`).
- **The time budget is honoured inside a pump.** A bind chunk is sized from the measured cost per asset (`AssetTable::bind_cost`), not a fixed 16: one material bind can cost milliseconds.
- **One pumper.** A render loop pumps once per frame (`SceneRenderer::upload`, nearest the camera); a tick leaves it alone (`TickLoop::render_loop_pumps_assets`), and the windowed frame no longer pumps before the renderer does.
- **Loader threads are half the cores** (`asset_load_workers`), so cooking does not preempt the frame, the renderer's encoder threads and physics.
- **Texture uploads are staged.** `resident_slot_staged` (the materials plugin's bind) allocates the slot and creates the GPU
  texture, but does not copy: `TextureStore::register_mips_staged` queues the cooked mips (`MipSource`, held until the upload
  is whole) and `World::advance_texture_uploads` (called by `scene::pump_assets` after the binds) writes whole block rows
  of the nearest waiting texture first, until `streaming.texture_upload_budget_kb` (4096) or `_ms` (1) is spent; a band is
  sized from the time left and the rate measured so far, so the clock is not overrun by one big write. A fresh slot is
  vacant (a material's fallback) until its texture is whole; a slot that already shows the coarse tier keeps it, and the
  finer texture replaces it in one step when complete (`replace`). The device-wide shared entry is published on
  completion, so another scene never samples half a texture. `resolve_assets_blocking` and `resident_slot` (the blocking
  form) still finish everything inside the call.
- **One asset's dependents are bound over several pumps.** An asset shared by thousands of entities was one indivisible bind
  (49 to 160 ms for a mesh): `bind_dependents` checks the pump's deadline after each entity and keeps the rest in
  `partial_binds`, which the next pump resumes first (`AssetPumpReport::pending` counts them; a new bind of the asset
  supersedes them).
- **Level hydration is incremental all the way.** `LevelHydration::step` spends its budget in units of one object (validate,
  spawn, components), 128 objects (frame and content-space props) or 16 class roots; an object whose components take over
  8 ms is named in a warning. Its prepare half also opens the project file index (components that resolve asset ids
  through it find it ready).
- Still on the frame thread: the renderer's first-use work (pipeline creation, owned by the renderer), the SceneDB mirror's
  scatter writes for rows a step dirtied (10 to 21 ms at worst), and the per-instance copy of mesh geometry in a static
  mesh component's bind (`GAMEPLAY.md`, issue 8).

What to watch when running the app on a texture-heavy level: the level textures
appear blurry within moments and sharpen nearest-first; frame time stays flat
while they do (Tasks window, `scene::pump_assets` scope).
`plugins/installed/tests/texture_heavy_level_probe.rs` (PROBE_PROJECT/PROBE_LEVEL,
optional PROBE_FRAME_BUDGET_MS) now streams the level and prints median/p99/worst
frame time and when assets settled. It was **not run**: no texture-heavy project
was available here, and the workspace did not build `installed_plugins` while
`plugins/vendor/materials` had other changes in progress.

Not done: meshes have no coarse tier (their loaders produce one form;
authored LODs are chosen per view by `LODComponent`, `.agents/LOD.md`, not by the
loader); a texture's GPU copy is replaced when its finer version is whole, not
patched mip by mip (the coarse tier shows until then); the material rebind that follows a bind
still runs on the frame thread.

## Still blocking or not yet done

- `standalone::select_level_pipeline` still reads the level and may build
  render graphs on the event loop before the renderer exists (the renderer
  takes its pipeline at construction).
- Headless `standalone::load_level` is blocking by design (no UI).
- Not audited: Play-In-Editor snapshot capture (`enter_play_mode`), the
  `class_reload_events` scan in `pie.rs`, `ui_loading_screen` startup tasks.
- No loading screen in the windowed game: it renders an empty world until the
  level lands.
- The `render_bps` change is in a submodule (`Plugin_RenderBps`): it needs its
  own PR and a re-pin.

## Tests

SceneDB: `tests/asset_budget.rs`; `helio_component` `a_large_image_loads_coarse_first...`; `engine_backend` `asset_pump_tests`.
`sublevel_display::tests::a_deferred_sync_asks_a_worker_and_applies_the_level_when_it_is_ready`
and the existing sublevel and `runtime_level` tests. Not run: the
`plugins/installed/tests` integration suite and anything that opens a window.

## Saving

The mirror of loading: a save never runs on a thread that draws or answers
input (`ui_level_editor::ui::save`).

- **Snapshot, then write.** `save_in_background` runs one tracked editor task
  ("Save level", category `Save`, see `.agents/EDITOR_TASKS.md`): scan the class
  registry (no lock), `snapshot_level` under a *read* lock on the shared scene,
  then `write_level` with no lock. The UI thread only awaits the task's result.
  Play-In-Editor's `play.level` is written the same way, inside its build task.
- **Atomic.** `write_level` goes through `virtual_fs::write_file_atomically`:
  a staged `.<name>.<pid>.<n>.tmp` beside the level, `sync_all`, rename over
  it. A crash or a failed write keeps the old file (a failed one removes its
  staging file; one left by a crash is ignored by every reader).
- **Coalesced.** `write_coalesced` allows one write per path. A save arriving
  meanwhile waits as the path's pending snapshot; a newer one replaces an older
  waiting one (latest wins), and every save it covers gets its outcome. A
  snapshot older than one already on disk is `Superseded`.
- **Own writes.** `is_own_write(path)` is true while the editor is writing the
  path or while the file still has the size and mtime the editor last wrote.
  Any reload-on-change path for the open level must check it first. None exists
  today (the editor has no level file watcher; `engine_fs::events` only drives
  asset and file-manager refreshes).

Tests (`ui::save::tests`): concurrent saves leave one whole file and no staging
file; a save during a write is coalesced, latest wins; a write that fails keeps
the old file and releases the path.

Left: `default_level::seed` (`save_to_file_with_editor_camera`) is a one-off at
project creation and still serializes inline; `scene_edit` test helpers use the
synchronous `save_to_file` on purpose.
