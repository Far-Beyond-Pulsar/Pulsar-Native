# Voxel worlds

The voxel plugin (`pulsar.voxel`, `plugins/vendor/voxel`, its own repository)
documents itself:

- `plugins/vendor/voxel/feature/README.md`: the system (grid, terrain field,
  edits, chunk octree, invariants, measuring, tests) and, under "Load-in, CPU
  queries and failures", what a level's voxel load-in costs in the editor
  (measured), what CPU world queries cost, how failures are reported, and
  what is open.
- `plugins/vendor/voxel/feature/docs/chunk-octree.md`: the renderer's design.
- `plugins/vendor/voxel/world/docs/`: features, planet maps, the two-rate
  field, volumes.

What a host should know (2026-11-25, details and numbers in the README):

- A new voxel world draws its first chunk when its pipelines are compiled;
  generation after that is nearest first within a GPU budget (about 2 s to a
  settled view). In the editor the first chunk came 24.6 to 25.1 s after the
  project opened (46.2 to 46.8 s before the plugin compiled a bundle's
  pipelines side by side): 15.5 s of that is the renderer coming up, 9 s the
  voxel bundles compiling one after the other on one of the task queue's two
  long-task workers (`editor_task_queue`, `DEFAULT_DEDICATED_WORKERS`), the
  other being held by "Rebuild render pipeline".
- Measure it again without a profiler: run the editor with
  `PULSAR_VOXEL_STATS=1` and read `engine.log` for the `Compile voxel ...`
  tasks (queued, started, finished), `VOXEL_WORLD`, the first `VOXEL_STATS`
  line with `drawn_chunks` and the first with `settled=true`, and the
  `Slow UI work` tasks for UI-thread holds.
- The packaged game's load-in has not been measured.

Update (2026-11-25, editor open round 5, `.agents/GAMEPLAY.md` issue 22):

- The voxel node reads `helio_core::BuildHints` (the renderer's published payloads, and whether this is its
  first build) and calls `ChunkPass::precompile`, so the world exists and holds the scene's volumes while the
  graph builds: the draw, generation and ray bundles compile in the build's first half second, beside the
  graph's own pipelines, not when the first frame finds the volume. A rebuild's pass inherits the running
  world and starts nothing.
- Measured in the editor (Direct3D 12, this machine, the Earth project): first chunk 21.4 and 22.4 s before,
  10.8 to 11.5 s after; settled 23.4 and 24.9 s, then 13.3 to 13.5 s. What bounds it now is the generation
  bundle's longest kernel (`feature_block_candidates_main`, 6 s of DXC by itself; `tile_caves_main` 4.3,
  `tile_columns_main` 3.8, `tile_slope_heights_main` 3.2): splitting them is the next lever.
- A viewport whose only activity is ambient (terrain foliage wind, water) draws at `editor.viewport.ambient_fps`
  (default 15), not at the display's rate: `.agents/RENDER_RUNTIMES.md`, "Frame policy".
