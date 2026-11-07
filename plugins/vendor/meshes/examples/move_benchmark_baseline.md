# Object-move benchmark: baseline

Produced by `crates/examples/move_benchmark.rs` (`cargo run --release -p examples --bin move_benchmark`).
Rerun it after every change that touches the move path and compare against these tables.

Goal: moving one object must cost the same at any scene size. A stage whose time grows
with `objects`/`tris` in the `move_one` or `editor_resync` rows is a scaling bug.

## How this baseline was captured

* Adapter: Mesa lavapipe (software Vulkan, 4 CPU cores), `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`.
  The `gpu` and `rt_gpu` columns are software-rasterizer time and are only meaningful as *deltas
  between rows of the same size*; CPU columns (`update`, `flush`, `rt`, `render`) are representative.
  Rerun on real hardware for absolute GPU numbers.
* 320x180, 10 measured frames per row after 4 warmup frames, `--segments 6`, 64 point lights.
* Commands:
  ```
  move_benchmark --graph hlfs --sizes 1000,4000,16000,32000 --frames 10 --warmup 4 --segments 6
  move_benchmark --graph default --editor --no-ray-query --sizes 1000,4000,16000,32000 --modes ss --frames 10 --warmup 4 --segments 6
  move_benchmark --scene cathedral_large --frames 10 --warmup 4 --segments 6
  ```
  (`--no-ray-query` only because lavapipe's compiler crashes on ray-query pipelines.)

## Findings

1. **Helio's own per-row move path is flat.** `move_one` (one `World::get_mut`) costs ~0.06 ms of
   CPU (`update`+`flush`) at 1k and at 32k objects, in both the HLFS graph and the editor's default graph.
2. **The editor's per-frame resync is O(scene).** `editor_resync` reproduces Pulsar-Native's
   `engine_backend::scene::sync_static_mesh_rows`, which re-inserts a `StaticObjectComponent` for
   *every* mesh entity on every frame the scene revision changes (every frame of a gizmo drag).
   SceneDB does not skip unchanged values, so every row is re-marked dirty and re-uploaded
   (236 B/row, ~7.5 MB/frame at 32k objects). CPU `update`+`flush`: 0.7 ms @1k -> 10.7 ms @32k,
   in a release build and before the editor's additional per-entity work (Transform lookups,
   GPU-handle lookups, two full `query().count()` scans for a `tracing::info!`). It also resets
   `prev_transform`, so the dragged object gets zero motion vectors.
3. **The RT acceleration path is O(scene) every frame, moving or not.**
   `SceneDbRayTracing::prepare` re-walks every caster and re-hashes every referenced mesh's full
   vertex+index data each call: 6.4 ms/frame idle on the large cathedral (416k tris, 16 objects),
   9.6 ms/frame idle at 32k objects. Any transform change then rebuilds the whole TLAS
   (`rt` 9.6 -> 16.6 ms CPU at 32k, plus the full TLAS build on the device).
4. **The editor only renders while something changes** (`is_idle` early-out in the editor
   renderer), so any fixed per-frame render cost is only *visible* while dragging.

## `grid_hlfs`

Adapter: llvmpipe (LLVM 20.1.2, 256 bits) (Vulkan), graph hlfs, 320x180, 10 measured frames per row (median ms; total also p95)

| scene | objects | tris | lights | mode | workload | update | flush | rt | rt_gpu | render | gpu | total | total p95 |
|---|---:|---:|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| grid | 1001 | 144012 | 64 | ss | idle | 0.00 | 0.02 | 0.00 | 0.00 | 3.13 | 185.37 | 188.53 | 208.82 |
| grid | 1001 | 144012 | 64 | ss | move_one | 0.01 | 0.04 | 0.00 | 0.00 | 3.36 | 176.45 | 179.73 | 209.15 |
| grid | 1001 | 144012 | 64 | ss | editor_resync | 0.28 | 0.46 | 0.00 | 0.00 | 3.59 | 186.33 | 190.80 | 200.38 |
| grid | 1001 | 144012 | 64 | rt | idle | 0.00 | 0.02 | 0.90 | 0.09 | 2.81 | 208.47 | 212.33 | 236.49 |
| grid | 1001 | 144012 | 64 | rt | move_one | 0.01 | 0.05 | 1.25 | 15.32 | 3.34 | 208.12 | 227.95 | 236.49 |
| grid | 1001 | 144012 | 64 | rt | editor_resync | 0.31 | 0.53 | 0.89 | 15.40 | 3.05 | 204.42 | 226.10 | 240.97 |
| grid | 4001 | 576012 | 64 | ss | idle | 0.00 | 0.02 | 0.00 | 0.00 | 4.48 | 265.01 | 269.50 | 289.36 |
| grid | 4001 | 576012 | 64 | ss | move_one | 0.01 | 0.05 | 0.00 | 0.00 | 4.50 | 269.05 | 272.16 | 312.84 |
| grid | 4001 | 576012 | 64 | ss | editor_resync | 0.96 | 0.81 | 0.00 | 0.00 | 4.44 | 266.29 | 271.58 | 298.41 |
| grid | 4001 | 576012 | 64 | rt | idle | 0.00 | 0.02 | 1.82 | 0.13 | 3.63 | 305.91 | 311.44 | 318.94 |
| grid | 4001 | 576012 | 64 | rt | move_one | 0.01 | 0.06 | 2.86 | 47.31 | 3.49 | 296.34 | 347.95 | 370.09 |
| grid | 4001 | 576012 | 64 | rt | editor_resync | 1.01 | 0.93 | 2.47 | 44.73 | 3.73 | 290.30 | 344.33 | 380.79 |
| grid | 16001 | 2304012 | 64 | ss | idle | 0.00 | 0.02 | 0.00 | 0.00 | 4.54 | 566.89 | 572.43 | 595.17 |
| grid | 16001 | 2304012 | 64 | ss | move_one | 0.01 | 0.06 | 0.00 | 0.00 | 4.79 | 594.82 | 600.04 | 676.44 |
| grid | 16001 | 2304012 | 64 | ss | editor_resync | 3.82 | 1.91 | 0.00 | 0.00 | 3.54 | 568.00 | 577.55 | 612.68 |
| grid | 16001 | 2304012 | 64 | rt | idle | 0.00 | 0.02 | 5.12 | 0.13 | 3.76 | 609.35 | 618.84 | 628.34 |
| grid | 16001 | 2304012 | 64 | rt | move_one | 0.01 | 0.06 | 9.00 | 73.76 | 5.48 | 601.44 | 690.84 | 722.71 |
| grid | 16001 | 2304012 | 64 | rt | editor_resync | 3.70 | 3.15 | 8.29 | 75.27 | 5.35 | 589.01 | 678.35 | 718.61 |
| grid | 32001 | 4608012 | 64 | ss | idle | 0.00 | 0.03 | 0.00 | 0.00 | 3.31 | 968.06 | 970.97 | 991.19 |
| grid | 32001 | 4608012 | 64 | ss | move_one | 0.01 | 0.05 | 0.00 | 0.00 | 4.27 | 969.57 | 974.43 | 1024.06 |
| grid | 32001 | 4608012 | 64 | ss | editor_resync | 7.37 | 3.42 | 0.00 | 0.00 | 5.55 | 969.66 | 986.32 | 999.28 |
| grid | 32001 | 4608012 | 64 | rt | idle | 0.00 | 0.02 | 9.61 | 0.13 | 5.43 | 1016.89 | 1031.24 | 1071.10 |
| grid | 32001 | 4608012 | 64 | rt | move_one | 0.01 | 0.06 | 16.64 | 103.34 | 6.63 | 1016.34 | 1146.19 | 1174.40 |
| grid | 32001 | 4608012 | 64 | rt | editor_resync | 7.67 | 4.12 | 16.42 | 107.74 | 5.51 | 1023.03 | 1171.44 | 1219.61 |

## `grid_default`

Adapter: llvmpipe (LLVM 20.1.2, 256 bits) (Vulkan), graph default (editor mode), 320x180, 10 measured frames per row (median ms; total also p95)

| scene | objects | tris | lights | mode | workload | update | flush | rt | rt_gpu | render | gpu | total | total p95 |
|---|---:|---:|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| grid | 1001 | 144012 | 64 | ss | idle | 0.00 | 0.02 | 0.00 | 0.00 | 3.10 | 147.15 | 150.60 | 163.66 |
| grid | 1001 | 144012 | 64 | ss | move_one | 0.01 | 0.05 | 0.00 | 0.00 | 2.68 | 146.67 | 149.52 | 157.94 |
| grid | 1001 | 144012 | 64 | ss | editor_resync | 0.25 | 0.44 | 0.00 | 0.00 | 3.31 | 143.81 | 147.10 | 163.14 |
| grid | 4001 | 576012 | 64 | ss | idle | 0.00 | 0.02 | 0.00 | 0.00 | 4.57 | 257.71 | 262.24 | 273.38 |
| grid | 4001 | 576012 | 64 | ss | move_one | 0.01 | 0.05 | 0.00 | 0.00 | 2.79 | 245.42 | 248.03 | 254.77 |
| grid | 4001 | 576012 | 64 | ss | editor_resync | 0.95 | 0.72 | 0.00 | 0.00 | 4.45 | 247.65 | 253.54 | 272.55 |
| grid | 16001 | 2304012 | 64 | ss | idle | 0.00 | 0.03 | 0.00 | 0.00 | 2.87 | 636.32 | 640.08 | 696.88 |
| grid | 16001 | 2304012 | 64 | ss | move_one | 0.01 | 0.05 | 0.00 | 0.00 | 4.27 | 636.17 | 640.76 | 656.37 |
| grid | 16001 | 2304012 | 64 | ss | editor_resync | 3.78 | 1.79 | 0.00 | 0.00 | 4.37 | 639.81 | 653.05 | 692.32 |
| grid | 32001 | 4608012 | 64 | ss | idle | 0.00 | 0.02 | 0.00 | 0.00 | 2.71 | 1068.20 | 1070.95 | 1127.57 |
| grid | 32001 | 4608012 | 64 | ss | move_one | 0.01 | 0.05 | 0.00 | 0.00 | 2.94 | 1102.68 | 1105.59 | 1141.90 |
| grid | 32001 | 4608012 | 64 | ss | editor_resync | 7.30 | 3.37 | 0.00 | 0.00 | 5.38 | 1071.62 | 1088.31 | 1119.34 |

## `cathedral_large_hlfs`

Adapter: llvmpipe (LLVM 20.1.2, 256 bits) (Vulkan), graph hlfs, 320x180, 10 measured frames per row (median ms; total also p95)

| scene | objects | tris | lights | mode | workload | update | flush | rt | rt_gpu | render | gpu | total | total p95 |
|---|---:|---:|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| cathedral_large | 16 | 416324 | 12 | ss | idle | 0.00 | 0.02 | 0.00 | 0.00 | 2.78 | 238.89 | 241.62 | 250.04 |
| cathedral_large | 16 | 416324 | 12 | ss | move_one | 0.01 | 0.05 | 0.00 | 0.00 | 2.81 | 236.63 | 239.77 | 262.16 |
| cathedral_large | 16 | 416324 | 12 | ss | editor_resync | 0.02 | 0.39 | 0.00 | 0.00 | 2.63 | 247.94 | 250.76 | 298.90 |
| cathedral_large | 16 | 416324 | 12 | rt | idle | 0.00 | 0.02 | 6.43 | 0.13 | 2.78 | 323.68 | 333.25 | 355.20 |
| cathedral_large | 16 | 416324 | 12 | rt | move_one | 0.01 | 0.05 | 6.57 | 5.76 | 3.49 | 338.75 | 355.16 | 366.66 |
| cathedral_large | 16 | 416324 | 12 | rt | editor_resync | 0.02 | 0.54 | 6.27 | 5.34 | 3.89 | 314.01 | 329.75 | 347.75 |


## After: mesh `Movability` + per-frame mesh/material dedupe in `SceneDbRayTracing`

`helio_core::Movability` now has `Static` / `Stationary` / `Movable` / `Dynamic`
(`can_move()`, `can_deform()`). A mesh entity carrying a non-deforming `Movability`
(anything but `Dynamic`) builds its BLAS once and is never re-hashed; an untagged mesh keeps
per-frame content invalidation. Independently of tagging, `prepare` now resolves each mesh's
GPU ranges and each material's RT properties once per frame instead of once per object.

`rt` column (CPU ms, median, RT mode, same settings as above), captured with
`--mesh-movability none` (untagged) and `--mesh-movability static`:

| scene | objects | tris | workload | baseline | after, untagged | after, `Static` meshes |
|---|---:|---:|---|---:|---:|---:|
| cathedral_large | 16 | 416k | idle | 6.43 | 6.45 | **0.49** |
| cathedral_large | 16 | 416k | move_one | 6.57 | 6.40 | **0.54** |
| grid | 1k | 144k | idle | 0.90 | 0.76 | 0.65 |
| grid | 16k | 2.3M | idle | 5.12 | 3.59 | 3.36 |
| grid | 16k | 2.3M | move_one | 9.00 | 7.50 | 7.20 |
| grid | 32k | 4.6M | idle | 9.61 | 6.08 | 6.01 |
| grid | 32k | 4.6M | move_one | 16.64 | 13.39 | 12.65 |

The grid shares 64 small meshes, so tagging matters little there; its remaining cost is the
per-object walk (every caster's transform must reach the TLAS) and, on a move, wgpu's full TLAS
instance upload and rebuild (`rt_gpu`), which is O(instances) inside wgpu itself. Updating only
the changed TLAS slots in `TlasManager` was tried and measured no difference, so it was not kept.

## Round 2: change journals, light scaling, object-batch skip

Same machine and settings as above unless noted. SceneDB gained multi-reader
change journals (`World::open_change_cursor`/`read_changes`), per-buffer
`BufferHandle::row_capacity()`/`content_generation`, and clears a
component's GPU row on remove/despawn.

### RT preparation (`rt` column, CPU ms, meshes untagged)

`SceneDbRayTracing::prepare` now reads SceneDB change journals: idle frames
do no per-object work and transform-only moves patch just those TLAS
instances. The remaining `move_one` cost is wgpu's full TLAS rebuild, which
re-serializes every instance (wgpu 30 has no TLAS refit).

| scene | objects | workload | baseline | now |
|---|---:|---|---:|---:|
| cathedral_large | 16 | idle | 6.43 | **0.01** |
| cathedral_large | 16 | move_one | 6.57 | **0.52** |
| grid | 1k | idle | 0.90 | **0.01** |
| grid | 1k | move_one | 1.25 | **0.81** |
| grid | 4k | idle | 1.82 | **0.01** |
| grid | 4k | move_one | 2.86 | **1.55** |
| grid | 16k | idle | 5.12 | **0.01** |
| grid | 16k | move_one | 9.00 | **4.75** |
| grid | 32k | idle | 9.61 | **0.01** |
| grid | 32k | move_one | 16.64 | **7.99** |

### Moving lights (default/editor graph, 1k objects, every light moves every frame)

`--graph default --editor --no-ray-query --workloads idle,move_lights --lights N`.
Every light is now culled and shaded (previously only rows below 64 in the
default graph). CPU cost of the moves (`update` + `flush`):

| lights | update + flush (ms) |
|---:|---:|
| 64 | 0.33 |
| 1024 | 0.65 |
| 4096 | 1.49 |

The HLFS RT probe (`gpu_hlfs_rt::benchmark_rt_resolution_and_acceleration`,
1,024 moving lights) measured host-side 0.03 ms to upload the lights, 0.08 ms
to build 10k moving instances and ~3 ms to encode/submit their TLAS; its GPU
numbers on lavapipe (seconds per frame) are software ray-query emulation.

### Object batch on idle frames (default/editor graph, 32k objects)

The sort/group/range pipeline is skipped while `static_objects` and
`materials` are unchanged (SceneDB content generation), after one settling
frame:

| workload | ObjectBatch CPU (ms) | render CPU (ms) |
|---|---:|---:|
| move_one | 1.77 | 4.30 |
| idle | **0.45** | **2.22** |

### Correctness fixes found along the way

* Default graph lit at most 64 lights (light culling, deferred, forward,
  transparent) while other passes read 256 rows of a 64-row buffer; all now
  use the buffer's live row capacity.
* `ShadowMatrixPass` kept the lights buffer bound at construction after
  SceneDB grew it; several bind-group caches keyed buffers by handle address.
* Motion vectors used the authored `prev_transform`, so an object that stopped
  moving kept its last velocity forever; the object batch now supplies the
  previous frame's transform.
* Despawned objects kept drawing and removed lights kept shining (SceneDB left
  their GPU rows untouched).
