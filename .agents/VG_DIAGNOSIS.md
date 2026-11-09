# VG diagnosis: #1599 / #1598 / #1417

Read-only investigation (worker C2, 2026-11-08). No source was changed. All paths are
under `plugins/vendor/meshes/` unless stated. The owner decides the final VG design; this
file lists causes, evidence and options only.

## 0. What was and was not verified

| Item | Status |
|---|---|
| Issue text (#1599, #1598, #1417) | Read. #1598 and #1417 are folded into #1599. #1417 has an empty body. |
| Code reading of cull, density control, builder error metric, tier classification, streaming | Done (file:line below). |
| `virtual_geometry_dense_acceptance` run on a GPU | **Not run.** The GPU exists (NVIDIA RTX Spark, aarch64 Windows), but `cargo test -p ui_level_editor --test virtual_geometry_dense_acceptance --no-run` rebuilds the whole editor dependency graph from scratch (228 crates after about 17 minutes, competing with other workers' cargo jobs, no sign of finishing soon). Killing the build was denied by the permission layer; it later stopped on its own with a compile error in `helio-pass-deferred-light` (E0599, one error; not investigated, likely a concurrent edit by another worker), so the test binary was never produced. |
| Debug views 107 (page residency) and 109 (error vs tau) | **Not captured.** The acceptance test never selects them (it only uses `DenseMask` 106 and `RasterTier` 103), so capturing them needs a harness or example change, which was out of scope. |

So everything below is code-reading inference unless it says "verified". Section 3 gives the
exact commands that would turn the inference into measurement.

Command to run the suite (the test lives in `crates/editor/ui_level_editor/tests/virtual_geometry_dense_acceptance.rs`,
package `ui_level_editor`, binary `virtual_geometry_dense_acceptance`; excluded from CI's `ci`
profile in `.config/nextest.toml:36`):

```
cargo test -p ui_level_editor --test virtual_geometry_dense_acceptance -- --nocapture
# RENDER_ACCEPTANCE_DUMP_DIR=<dir> writes vg_dense_*.png
```

## 1. #1598: why `dense_tiles` stays 0

### 1.1 Mechanism that makes dense tiles impossible without fine clusters (verified by reading)

- A tile becomes dense only from the **software raster's** counters (>= 64 overlapping
  triangles or >= 64 depth-passing hits per 16x16 tile; `feature/vg-raster/src/pass.rs:33-37`,
  defaults `:78-85`).
- Clusters the cull classifies as hardware-class (bins 0..3) are never counted by those
  counters and are "never moved to dense" (`feature/vg-cull/src/lib.rs:57`, doc). The cull only
  puts a cluster into the dense bin if it is already SW-class (`edge <= sw_edge_px`) and sits
  on a tile marked dense last frame (`feature/vg-cull/shaders/vg_cull.wgsl:390-395`), or is
  wholly covered by dense tiles (`:383`).
- SW class is `edge = 2.7 * r_px / sqrt(tris) <= 2.0`
  (`vg_cull.wgsl:390`, `feature/vg-cull/src/lib.rs:196`, `feature/vg-cull/src/core.rs:93`).
  `r_px` is the cluster's *sphere* radius, and for non-leaf clusters that sphere is the
  producing group's shared sphere (`vg/build/src/dag.rs`, ClusterNode doc, about line 76).
  A coarse cluster with few triangles and a large group sphere therefore gets a large `edge`
  and goes hardware (blue in the tier view).
- So the symptom in #1598 (tier view all hardware, dense overlap view near black, no dense
  tiles) is exactly what you see when the cut selects coarse clusters. Dense tiles can never
  start from a coarse cut: this is a chicken-and-egg that only a fine cut breaks.

### 1.2 Why the cut is coarse: three stacked density caps (code-verified existence, magnitude inferred)

The test builds 40 instances of one 33k-triangle noisy sphere (`virtual_geometry_dense_acceptance.rs`,
`noisy_sphere(110, 150)`, `CROWD = 40`) at 220..560 units, scale 0.18..0.3, at 256x256. Each
sphere is a few pixels across, so its finest level is far above any per-pixel triangle budget.
Since the VG-BUG-10 fix (2026-10-23, `.agents/plans/virtual-geometry.md:288-294`) the engine
deliberately keeps the selected density near 2 triangles per pixel. That is the opposite of
what this test wants (many overlapping sub-pixel triangles). Three places enforce it:

1. **Runtime screen-wide density control, on by default.**
   - Defaults `selection_triangles_per_pixel = 2.0`, `selection_sensitivity = 1.0`
     (`vg/stream/src/runtime.rs:52-62`, duplicated in `feature/vg-cull/src/core.rs:79-95`).
   - Budget = `2.0 * 0.75 * width * height` triangles (`feature/vg-cull/src/pass.rs:199-205`),
     about 98k triangles at 256x256, divided by the learned visible fraction.
   - `cs_density` (`vg_cull.wgsl:667-728`) sums each instance's predicted triangles over
     tolerance multipliers 1..8 and picks the smallest multiplier that fits. The crowd
     predicts about 1.3M triangles at multiplier 1, so the multiplier rises to the cap
     (`MAX_SCALE = 8`, `vg/format/src/density.rs:23`) at up to 1.25x per frame
     (`:726`, `density.rs:66`). `load_inst` multiplies every instance's tau by it
     (`vg_cull.wgsl:221`), so effective tau goes to about 8 px at default tau 1, and about 2 px
     even when the test asks for tau 0.25.
   - The test cannot opt out: `host_part.rs:76-95` builds the runtime settings from
     `VirtualGeometrySettings` and fills the density fields from `VgRuntimeSettings::default()`
     (`host_part.rs:93`). The only off switch is the env var `HELIO_VG_SELECTION_SENSITIVITY=0`
     (read in `runtime.rs:58-62` and `core.rs:85-91`). Note `selection_sensitivity` is clamped to
     0..1 (`pass.rs:160-164`), so "sensitivity 8" in the issue text can only be the *dense
     classifier's* `HELIO_VG_DENSE_SENSITIVITY` (range 0.125..8, `vg-raster/src/pass.rs:57-65`),
     which lowers the 64-triangle/64-hit thresholds. That knob does nothing while the clusters
     are hardware-class.
2. **Builder-baked density relaxation.** `vg/build/src/dag.rs:38-49, 394-446`:
   `DENSITY_TARGET = 1.5`, `DENSITY_TOLERANCE_GAIN = 12`. A group's stored error is relaxed up to
   12x below its measured deviation once the level would exceed 1.5 triangles per silhouette
   pixel. The relaxation is a property of the cooked asset; no runtime setting undoes it
   (needs a re-cook, `COOK_REVISION`).
3. **Residency-gated cut.** `vg_cull.wgsl:459`: `draw = p_err > tau && (s_err <= tau || !kids_resident)`.
   Until the child page is resident the parent stands in, and the child is requested only when
   `s_err > tau` (`:450-452`). With the inflated tau, `s_err > tau` is rarely true for these
   small spheres, so fine pages are rarely even requested. (Inferred: page-residency view 107
   would show root-only pages resident; this is the exact thing the issue asked to look at.)

Together: effective tau is the user tau times 1..8 (runtime), and the stored error ladder is
itself relaxed up to 12x (builder). Both exist to keep triangles per pixel below about 2, which
is why no setting of the dense-classifier sensitivity can help.

### 1.3 Other contributors (lower confidence)

- The root shortcut (`vg_cull.wgsl:~620-640`, `try_root_shortcut`) emits only the root clusters
  when every root cluster's `self_error` projects under tau; with an inflated tau this is likely
  taken for the far crowd. It is the same cut, just reached earlier.
- The dense-mask feedback loop is a known hazard: dense-bin clusters starve the raster counters
  that classify the tile (the test's third test is the regression for it,
  `virtual_geometry_dense_acceptance.rs` bottom). Not the cause of dense never starting, but
  it will matter once tiles do go dense.
- Plan doc, `.agents/plans/virtual-geometry.md:279`: on an RTX 4090 Tier 3 does not beat Tier 2
  at any measured density, and `tier3` defaults to off. The suite forces `tier3: true`. So the
  suite asserts a tier that the project's own policy says rarely pays.

### 1.4 Most likely root cause (inferred, not yet measured)

The dense tier is starved by the density cap, not by a streaming failure: the VG-BUG-10 density
control (runtime multiplier up to 8x plus the builder's 12x relaxation) selects a coarse cut for
this crowd, the coarse clusters classify as hardware raster, hardware clusters never feed the
dense counters, and `dense_tiles` stays 0. The acceptance test was written for the pre-density-
control behaviour and was never reconciled with the 2026-10-23 policy. "Fails as on p16" fits:
the split did not change the behaviour.

Streaming never delivering fine pages is a *secondary* hypothesis. Nothing in the reviewed
scheduler (`vg/stream/src/scheduler.rs`) prevents it (budget 512 MiB default, one small asset),
but requests are issued only when `s_err > tau` (see 1.2 item 3), so the density cap also
suppresses them.

## 2. #1417: why quality transitions are visible (code analysis, no measurement)

The cut is per cluster with a hard threshold and no temporal or spatial blending. Sources of
perceptible change, in rough order of likely visibility:

1. **Global tau multiplier changes with the view.** `cs_density` re-solves a screen-wide
   multiplier every frame, rate-limited to 0.96x..1.25x per frame (`vg_cull.wgsl:726`). Moving
   the camera, a new instance entering the frustum, or the learned `VisibleFraction` changing
   (`vg/format/src/density.rs:80-120`, `pass.rs:199-205`) moves tau for every instance at once,
   so LOD pops can happen across the whole screen with no relation to any one object's distance.
2. **Stored error relaxed up to 12x at cook time** (`dag.rs:445`). The error ladder is no longer a
   bound on deviation, so a cluster swap can differ by far more than tau pixels, then the
   runtime multiplier can add up to 8x on top. Visible as pops at modest distances. Plan doc
   `VG-BUG-4` (floating-piece error not a conservative bound) is the same class.
3. **Residency-driven swaps.** When a child page arrives or a page is evicted the cut changes
   (`vg_cull.wgsl:459`). Page pool thrash is already logged (VG-BUG-2, `.agents/plans/virtual-geometry.md:280`).
   Nothing smooths the change.
4. **Position-only error.** The measured error is point-to-surface distance
   (`vg/build/src/deviation.rs`); normals, UVs, material boundaries and silhouettes are not
   measured, so shading can change when positions barely do.
5. **Hard tier switches.** The SW/HW split at `edge <= sw_edge_px` (`vg_cull.wgsl:390-395`) and
   the dense-tile mask use no hysteresis on the edge test (the mask itself has latch and
   hysteresis, `vg-raster/src/pass.rs:38-42`), and the two rasterisers differ in coverage rules.
6. **Cluster-, not triangle-granular.** The cut selects whole clusters (up to 128 triangles)
   with a shared group sphere, so one decision flips many triangles. The issue title says
   "triangle selection"; the code has no triangle-level selection.
7. **Where nothing helps:** no dithered cross-fade, no geomorphing, no temporal accumulation
   of the LOD choice. The cut test is the only mechanism.

### Relation between #1598 and #1417

Both are driven by the same decision: how selected density is controlled. Fixing the
acceptance test by weakening the density control would make #1417's problems different
(fewer global multiplier pops, more triangles per pixel); fixing #1417 by freezing tau would
make the dense-tier acceptance easier. They need one design, which matches folding them into
#1599.

## 3. How to turn inference into measurement (no source changes needed for the first two)

1. Wait for/perform the build (it is heavy: the first build of `ui_level_editor` tests; expect
   an hour or more on this machine with other cargo jobs running, then fast incremental runs).
2. A/B on the existing test, unmodified, by environment only:
   - baseline: `cargo test -p ui_level_editor --test virtual_geometry_dense_acceptance -- --nocapture`
   - density control off: `HELIO_VG_SELECTION_SENSITIVITY=0` (prediction: `dense mask` pixels
     appear, `dense_tiles_processed > 0`; if it still fails, the builder's relaxation, root
     shortcut or streaming is the remaining cause).
   - dense classifier more eager: `HELIO_VG_DENSE_SENSITIVITY=8` (prediction: no change while
     the density control is on).
   The test prints `NonRT: covered pixels`, `dense tile mask: N pixels (M tiles)` and the
   `tier3_stats()` struct (`frames_nonrt`, `dense_tiles_processed`, and so on).
3. For the two views the issue named (107, 109) a harness is required: the example
   `examples/vg_stress_capture.rs` (accepts per-shot `tau_px`, line ~283) or a copy of the
   acceptance harness that calls `mailbox.set_viewport_debug_mode(VgDebugView::PageResidency.id())`
   (id 107) and `ErrorVsTau` (id 109) (`vg/format/src/debug_view.rs:46,50`). Predictions:
   107 shows only root pages resident (or only a small fraction); 109 shows projected error well
   under the effective tau everywhere because the effective tau is 1..8x the setting.

## 4. Options for the final VG structure (not chosen; owner decides)

These are independent axes; a final design picks one per axis.

**A. Where selected density is controlled**
- A1. Keep the screen-wide multiplier (current). Make the acceptance test and Tier 3 explicit
  about it: expose the density controls through `VirtualGeometrySettings` so a test or a
  project can set budget and sensitivity, and change the dense scene to one that is dense *within
  the budget* (many overlapping instances, not sub-pixel detail). Cost: dense tier only
  engages on overlap, not on detail.
- A2. Per-instance or per-cluster screen-error only (Nanite-style): no global multiplier, error
  threshold is the only LOD driver; density follows from geometry. Removes global pops (#1417 item 1) and
  the test conflict, but loses the hard triangle-per-pixel bound that VG-BUG-10 introduced for
  scans and foliage.
- A3. Make the budget act on a different quantity (triangles per *covered* pixel or per tile),
  so empty screen does not count and dense tiles are preserved deliberately: a tile classified
  dense is exempt from further coarsening (dense is then a first-class outcome of the budget,
  not a failure of it).

**B. How the builder bounds density**
- B1. Keep baked relaxation (`DENSITY_TOLERANCE_GAIN` 12). Cheapest at runtime; error is no longer
  a bound, so it should be stored separately.
- B2. Store two quantities per group: true deviation (monotone, geometric bound) and an optional
  "selection" error used only when a budget is active. The runtime picks one; the visual bound
  is kept.
- B3. Drop baked relaxation, rely on runtime control or on a coarser root and instance-level
  sub-pixel cull (VG-BUG-6).

**C. Transition quality (#1417)**
- C1. Keep hard cuts, make tau stable: no per-frame global multiplier, hysteresis on both
  the error test and the SW/HW edge test.
- C2. Temporal blend: dithered or stochastic cross-fade between the outgoing and incoming
  cluster over a few frames, which needs both to be selectable for a short window and a
  history/motion contract from the spine.
- C3. Geomorph or attribute-aware error: extend measured error with normal/UV deviation and,
  where affordable, morph vertices toward the parent. Highest cost in the builder and the format.
- C4. Triangle-granular selection inside a cluster (finer than the group), which changes the
  format and the rasteriser contract.

**D. Dense tier (Tier 3)**
- D1. Keep it as an opt-in mode (it already defaults off); move the acceptance suite to a profile
  that explicitly turns the density control off and says so.
- D2. Let hardware-class clusters feed the dense counters (a cheap estimate from the cull,
  without the SW raster), so classification no longer depends on the SW raster having run.
- D3. Remove Tier 3 if the owner's measurements (the plan doc's 4090 numbers) show no pay-off
  anywhere; keep the SW/HW tiers only.

**E. Streaming**
- E1. Keep demand-driven requests (only when `s_err > tau`). E2. Add a prefetch band
  (request children while `s_err > tau / k`) so fine pages are resident before the cut needs
  them, which also reduces residency-driven pops (#1417 item 3).

## 5. Required report (helio-architecture)

- **Design:** none built. This is a diagnosis only; the proposed axes above sit in the owners
  that already exist (`vg/build`, `vg/stream`, `feature/vg-cull`, `feature/vg-raster`,
  `feature/vg-dense`, `component/vg_settings`).
- **AAA comparison:** UE5 Nanite selects by per-cluster screen-space error against a fixed pixel
  threshold with no global density multiplier, and relies on hierarchical culling and a software
  raster for sub-pixel triangles; dense overlap is handled by its visibility buffer, not a
  separate tier. Our global multiplier (A1) has no equivalent there; option A2 is closest.
  Cross-fading in Nanite-class systems is mostly left to TAA; Unity/others use dithered LOD
  cross-fade for discrete LODs (C2).
- **Deleted:** nothing (read-only).
- **Verified:** read the code paths above and the plan doc. Not verified: the acceptance test
  was not run (build too heavy and contended), debug views 107 and 109 were not captured, no
  GPU frame was inspected. Every causal claim in sections 1 and 2 is inference from source.
