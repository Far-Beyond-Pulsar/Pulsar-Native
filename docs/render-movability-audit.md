# Movability across the renderer: audit and plan (#836, #835)

Status: **audit only**. Everything below is read off the code in the Helio
submodule (`crates/renderer/helio`, branch `feat/motion-gate`). Nothing here was
measured or run on a GPU, and the renderer changes are deliberately not made
blind: they need a machine with a real adapter (see "How to verify").

`helio_core::Movability` is `Static | Stationary | Movable | Dynamic` with
`can_move()` (Movable, Dynamic) and `can_deform()` (Dynamic only).

## Who reads it today

| Consumer | What it does with `Movability` |
|---|---|
| `helio-component` `MotionGate` (`motion_gate.rs`) | **Gameplay**: a script moving an object whose `Movability` is not `can_move()` is refused with a typed error (the `Transform` script methods consult every registered gate). |
| `helio-pass-hlfs` `SceneDbRayTracing` (`scene.rs`) | **BLAS invalidation**: a mesh with a non-deforming `Movability` skips the per-rescan content hash and is built once; a `Movability` change triggers a rescan (its own change cursor). |
| `helio-pass-object-batch` | The `INSTANCE_FLAG_MOVABLE` bit splits shadow draws into a *static* and a *movable* indirect buffer (`object_batch.wgsl`, `ObjBatch ShadowStaticIndirect` / `ShadowMovableIndirect`). **Who sets the bit from `Movability` is not visible in the passes**: the only writer found is the demo helper in `examples/v3_demo_common.rs`; the editor's projection (`StaticMeshComponent.movability` → SceneDB `helio::Movability` → instance flags) is the place to verify. |
| Light component | Carries `movability` as a reflected property and projects it into SceneDB (Pulsar-Native#837). No pass found that caches or bakes a `Stationary` light on that basis. |
| Everything else | No reference found: shadow atlases, light culling and cascade passes, virtual geometry, depth/g-buffer, and the transform upload path. |

## Per-pass plan (what #836 asks for)

Ordered by expected payoff; each item is one reviewable change with its own
benchmark.

1. **Object batch / transform upload (Static never re-uploads).** The row is
   written at placement. Make the SceneDB-to-GPU mirror skip dirty-marking for
   `Static` rows after the first flush and warn if one is written later (the
   `MotionGate` covers scripts, not physics or editor edits). Benchmark:
   `move_benchmark` with N static + M moving, cost must be flat in N.
2. **Shadow atlases.** The static/movable indirect split already exists in the
   object-batch pass; what is missing is *caching the static atlas pages*: render
   static casters into a persistent page, re-render only when a static caster or
   light changes, composite with the movable pass each frame. This is the largest
   win and the largest change (`helio-pass-shadow`, `helio-pass-shadow-dirty`).
3. **Light passes.** `Stationary` lights (fixed transform, animatable
   colour/intensity) can keep their static shadow page and only re-render the
   movable casters; `Static` lights can be fully cached. Depends on 2.
4. **HLFS / ray tracing.** See "TLAS" below.
5. **Virtual geometry.** Skip re-clustering unless `can_deform()`: the pass needs
   a per-mesh revision key like HLFS's BLAS revision above (`Movability` absent
   means "hash the content", present and non-deforming means "never").
6. **Validation.** Warn (once per entity) when a `Static`/`Stationary` entity's
   transform changes in play mode; stay silent in the editor. The scripting side
   already errors instead of warning.

"Each pass documents what it does per movability" is best done as a table in each
pass crate's module doc, filled in as each item lands.

## TLAS (#835)

Today: any frame with a moved object rebuilds the whole TLAS, because wgpu 30 has
no TLAS refit (BLASes are reused; a transform-only change rewrites just those
instance slots and still rebuilds the acceleration structure). Cost is therefore
linear in total instances, which is the thing the acceptance asks to remove.

Plan:

- Keep **two `TlasManager`s** in `SceneDbRayTracing`: `static_tlas` (`Static`
  and `Stationary` instances, rebuilt only when that set changes) and
  `dynamic_tlas` (`Movable`, `Dynamic`, and anything with no `Movability`,
  rebuilt each frame it moves). A moved object then rebuilds a TLAS sized by
  the dynamic count only.
- A ray query traverses one TLAS, so shaders that today bind a single
  acceleration structure must trace both (static first, dynamic second, keep the
  nearest hit). That is a shader and bind-group-layout change in every RT
  consumer of `SceneDbRayTracing::tlas()`; the accessor becomes a pair.
- Where wgpu later adds TLAS update/refit, collapse back to one TLAS and delete
  the split. Track the wgpu issue; the split is a workaround and should be
  written as one (a `TlasSet` type behind `tlas()`).
- Extend `move_benchmark` with a ray-query run on real hardware (the existing
  baseline is in `crates/examples/move_benchmark_baseline.md`); the acceptance
  test is "moving one object costs the same with 1 000 and 100 000 static
  instances".

## How to verify (needs a GPU)

- `cargo run -p examples --example move_benchmark --release` before and
  after, with and without ray queries, recording into
  `move_benchmark_baseline.md`.
- The HLFS RT tests (`passes/3d/helio-pass-hlfs/tests/gpu_hlfs_rt.rs`, including
  the `Movability::Static` / `Dynamic` cases) cover BLAS/TLAS correctness, but
  they need an RT-capable adapter.

I did not implement these: the changes are inside shaders and bind-group layouts
that can only be validated by running the renderer, which isn't possible in this
environment, and a blind change to the RT path risks breaking rendering for
everyone.
