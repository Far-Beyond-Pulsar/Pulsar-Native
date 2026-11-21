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

## Still blocking or not yet done

- **Asset streaming is not progressive.** `World::pump_assets` (SceneDB, a
  separate repo) applies every finished load and binds every dependent in one
  call under the write lock, starts all loads at once, in no priority order,
  and each asset is loaded at full quality. A large level can therefore spike
  a frame when many loads finish together, and nothing loads nearest-first or
  low-resolution-first. It needs a budgeted pump (`max_bound` / time budget),
  request priority from the camera, and progressive mips and mesh LODs
  (`.agents/MEMORY.md`, `.agents/LOD.md`). It spans SceneDB, Helio and the
  loaders.
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

`sublevel_display::tests::a_deferred_sync_asks_a_worker_and_applies_the_level_when_it_is_ready`
and the existing sublevel and `runtime_level` tests. Not run: the
`plugins/installed/tests` integration suite and anything that opens a window.
