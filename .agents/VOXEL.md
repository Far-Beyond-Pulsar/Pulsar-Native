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
