# VG museum example

From `crates/renderer/helio`, run:

```powershell
cargo vg-stress
```

`cargo vg-stress` is an alias (`.cargo/config.toml`) for
`cargo run -p examples --features vg-stress --bin vg_stress --release`. The
`vg_stress` binary requires the `vg-stress` feature; other examples and default
builds never download or compile any of the provisioning dependencies
(`ureq`, `sha2`). Close a running copy before rebuilding on Windows.

## First run: self-provisioning

The six Stanford scans (about 570 MB of downloads, about 1.2 GB of prepared
meshes), plus the Poly Haven marble and trees described below, are fetched and
prepared by pure Rust, with no Python, Blender, curl or
tar involved (`vg_stress_prep/`, shared by `build.rs` and the example):

1. **Build time.** With the `vg-stress` feature in a *release* build,
   `build.rs` checks `assets/vg_stress/meshes/prepared.json`. If any scan is
   missing it runs the provisioning (progress appears as `cargo:warning=` lines
   when the script finishes). Failure never breaks the build.
2. **Run time fallback.** If the meshes are still missing when the demo starts
   (prebuilt binary, debug build, failed download), the loader thread runs the
   same code first; progress is shown in the window title and console.
3. Downloads resume (`.part` files, HTTP Range), retry with back-off, try
   mirrors, and are checked against size and SHA-256. Archives are streamed
   through gzip/tar and PLY parsers (ascii, binary little/big endian). Each scan
   is stood up, centred, grounded and scaled exactly as the old Blender script
   did (no decimation), given smooth normals and written as
   `assets/vg_stress/meshes/<name>.glb`. Prepared outputs, sources and caches
   are git-ignored. The Stanford licence (non-commercial, credit Stanford
   Computer Graphics Laboratory) is printed when downloading and kept in
   `assets/vg_stress/CREDITS.md`.

Set `HELIO_VG_STRESS_SKIP_PROVISION=1` to stop the provisioning, at build time
(for example for `cargo check`) and at run time (the museum opens with what is
on disk: offline, or a download source unreachable). Delete `assets/vg_stress/meshes/prepared.json`
to force re-conversion; delete `assets/vg_stress/source` to force re-download.

### Your own meshes

The stress-grid helpers (`vg_stress_data/layout.rs`: `scan_mesh_folder`,
`stress_grid_layout`) take every `.fbx`, `.obj`, `.gltf` or `.glb` placed
directly in a folder (sorted by name; names starting with `_` or `.` are
skipped) and spread them over a seeded grid. Drop your files into
`assets/vg_stress/meshes/` (git-ignored) or any other folder you pass in.

The architecture kit (`kit/meshes/*.fbx`, `kit_manifest.json`) is committed
(original geometry). Marble and trees are real CC0 assets from Poly Haven,
provisioned on first run (next section); the procedural marble generator is
gone. This standalone example assembles the museum shell, columns, pedestals,
courtyard, props, marble materials, lights, the scan exhibits and the trees in
memory. It never creates a `.level`. Shared dimensions are in
`assets/vg_stress/museum_dims.json`.

## Poly Haven assets (all CC0)

Provisioned by the same pure-Rust code as the scans (`vg_stress_prep/polyhaven.rs`):
the Poly Haven API (`https://api.polyhaven.com/files/<id>` and `/info/<id>`, sent
with a `PulsarEngine-Helio-vg_stress/1.0` User-Agent as the API terms ask)
lists each asset's files, sizes and MD5s; glTF packages keep their directory
structure (`trees/<id>/<id>_2k.gltf`, `<id>.bin`, `textures/*.jpg`); texture sets go
to `marble/<id>/`. Downloads resume, retry, and are checked against size and MD5.
`vg_stress_prep/polyhaven_pins.json` pins the sizes and MD5s captured on
2026-10-22 (a mismatch with the live API only warns; the API stays authoritative).
Each asset gets a `polyhaven.json` manifest (resolution, files, authors) and a
`prepared.json` entry `ph:<id>`; bumping `PH_RECIPE_VERSION` (or the scan
`RECIPE_VERSION`, now 2) makes existing installs re-provision. The credits file
`assets/vg_stress/CREDITS_POLYHAVEN.md` (authors from the API) is regenerated
whenever something is downloaded. Everything downloaded is git-ignored.

| Asset | Use | Download | Notes |
|---|---|---|---|
| `grey_cartago_01` | main floor, all exhibit pedestals, impluvium | 78.5 MB | 8k jpg diffuse + nor_gl + arm (16k exists, not used) |
| `grey_cartago_03` | Ionic columns, benches, steles, secondary floor slabs, plinths | 84.9 MB | 8k |
| `marble_01` | Doric columns, entablature, cella walls, pediments, steps | 25.7 MB | 8k |
| `marble_mosaic_tiles` | floor inlay border (`marble_green` slot), plaza | 149.5 MB | 8k |
| `pine_tree_01` | hero and forest conifers | 982 MB (948,849,556 B .bin) | 2k, 3 variants, 17.18M tris |
| `fir_tree_01` | conifers | 510 MB | 2k, 3 variants, 6.98M tris |
| `island_tree_01/02/03` | courtyard accents, forest | 79.8 / 59.2 / 97.6 MB | 1.60M / 1.07M / 2.09M tris |
| `tree_small_02` | courtyard accents, forest | 115.3 MB | 2.06M tris |

If 8k is not offered the set falls back to 4k then 2k (consistently for all
three maps); the optional displacement maps are only fetched with
`VG_STRESS_DISPLACEMENT=1` (the renderer does not use them yet). Marble uses
the ARM map as the roughness/occlusion/metalness texture. Whichever file fails to
download, the material keeps a flat colour and the scene still opens.

### Choosing the tree set, density and texture budget

| Setting | CLI | Environment | Values |
|---|---|---|---|
| Trees | `--trees <v>` / `--small-trees` | `VG_STRESS_TREES` | `full` (default, about 1.5 GB), `small` (island trees + tree_small_02, about 0.35 GB), `none`, or a list of ids |
| Density | `--density <v>` | `VG_STRESS_DENSITY` | `low` 0.4, `normal` 1 (default), `high` 3, `insane` 8, or a number (clamped to 128x) |
| Textures | `--textures <v>` | `VG_STRESS_TEXTURES` | `low`, `balanced` (default), `max` |

Pass CLI flags after `--`: `cargo vg-stress -- --trees small --density high`. The
build script only sees the environment, so `VG_STRESS_TREES=small` also limits the
build-time download. The scene is seeded and deterministic; the startup log
prints the instance counts, the expected source triangles, GPU texture memory and
download size.

| Preset | Tree instances | Statue instances | Source triangles |
|---|---|---|---|
| `small` trees, density 1 | about 2,100 (8 hero, 96 courtyard, 2,000 forest) | 72 | not re-measured after the forest increase |
| default (full, density 1) | about 580 (8 hero, 48 courtyard, 520 forest; was 162 before the forest increase) | 72 (18 exhibits, 24 hall copies, 30 courtyard crowd) | 777 M (kit 1.7 M, statues 408 M, trees 368 M) |
| `high` (3) | 470 | 156 | 2.07 G |
| `insane` (8) | 1240 | 173 | 3.9 G |

Instances of one asset share the same GPU geometry (one mesh, one VG asset;
only an object row per instance). Hero trees stand along the approach, forests
fill the grounds beyond the plaza, the hall copies line the ambulatories and the
crowd stands on the plaza with seeded yaw and scale.

GPU texture budget at `balanced`: marble diffuse 8k, normal/ARM 4k with full mip
chains (CPU generated, sRGB-aware, normals renormalised), trees 2k diffuse and 1k
normal/ARM; about 2.6 GiB with all six trees (printed at startup). `low` halves
that; `max` uploads everything at its downloaded size (marble 8k throughout is
about 4 GiB). The kit's UVs repeat every 2 m, so the marble texel density is far
above the photographed scale (the tiles are 0.75 to 2 m): veins look larger than
life, the price of keeping the kit untouched.

### Alpha-masked tree parts

Virtual geometry draws alpha-tested materials but not blended ones, and
`VirtualGeometryHost` keeps blended surfaces classic. The tree
glTFs mark these materials `alphaMode: BLEND`:

| Tree | Masked slot | Masked triangles | Opaque (VG-eligible) |
|---|---|---|---|
| `pine_tree_01` | none (needles are opaque geometry) | 0 | all 17.18M: trunk, bark, dead branches, twigs/needles |
| `fir_tree_01` | `fir_tree_01_twig` | 6.72M of 6.98M | bark, trunk, dead branches |
| `island_tree_01/02/03` | `*_leaves` | 1.06M / 0.71M / 1.41M | trunk and branches |
| `tree_small_02` | `tree_small_02_leaves` | 1.94M of 2.06M | trunk and branches |
Each tree variant stays one glTF mesh per material slot (parts). The glTFs mark
the leaf/twig materials `alphaMode: BLEND` (never VG-eligible), so the museum
authors them as `FLAG_ALPHA_TEST | FLAG_DOUBLE_SIDED` (cutoff 0.5 via the
single-row fallback) and `MASKED_PARTS_USE_VG` is `true`: masked parts are
cooked and drawn through the masked VG raster like every other part, with no
classic stand-in. The leaf colour JPEGs carry no alpha; the separate
`*_alpha_*.png` shipped with the package is combined with the albedo into one
RGBA upload at load time (`decode_with_opacity`), so the alpha test samples the
albedo texture's alpha. Setting the flag to `false` hides masked parts (there is
no classic path). `KHR_texture_transform` UV scale/offset (bark, branches) is
baked into the vertices at import.

## Loading and scale (cook-first)

Virtual-geometry assets (scans, trees) are **never shown in a stand-in form**.
The window opens at once and renders the architecture kit (the only
always-classic geometry); the loader thread then works in this order: kit,
textures, assets whose caches are already valid, then the cook phase. An asset's
instances are created only after its `.vgeo` exists, and they draw only after the
streaming runtime reports its root pages resident (`select_visible` in
`vg_stress.rs`, unit tested). If a cook fails, one error is logged and nothing is
shown for that asset; there is no classic fallback. A device without the VG
passes shows no scans or trees (one warning).

* **Persistent and resumable.** `.vgeo` files in `assets/vg_stress/cache` are
  keyed by content id and validated against the format version and cook
  revision in their header; a valid file is never cooked again. Writes are
  atomic (unique temp file, fsync, rename), so a killed run leaves at most a
  stale `*.tmp` (removed on the next start) and never a corrupt cache. A small
  per-asset part record (`parts-v6-*.bin`: transform, slot, radius, content id,
  records) makes a warm start skip source parsing entirely.
* **Memory-bounded.** Source files are cooked as jobs (a file's variants share
  one import). Each job's peak is estimated from triangle counts (96 B/triangle
  imported + 200 B/triangle of the largest part). A gate admits jobs while the
  sum of estimates stays within the budget (60% of free RAM, clamped to
  2-24 GiB, `VG_STRESS_COOK_MEM_GB=<n>` overrides), at most one 10M+ triangle
  file (pine, Thai, Lucy) at a time, at most `cpus/4` (1-3) files at once, smallest
  first so cheap assets appear first. A job larger than the whole budget runs alone.
  `VG_STRESS_COOK_THREADS` sets the cook pool (default `cpus - 2`, 2-8).
* **Progress.** The title bar and the console show
  `Cooking VG k/N assets (done of total tris) | now: <asset (tris, time)> | elapsed | ETA`;
  per-asset start/finish lines and a 15 s heartbeat go to the console.
* **Forced recook.** `cargo vg-stress -- --force-recook` (or
  `VG_STRESS_FORCE_RECOOK=1`; combine with `--prepare` to do it headless) ignores
  every valid `.vgeo` cache and cooks it again, once per asset per run, then
  overwrites the cache. Without it a cache is rebuilt only when its cook revision
  is stale.
* **Headless prepare.** `cargo vg-stress -- --prepare` (or `VG_STRESS_PREPARE=1`)
  provisions the sources and cooks every cache with no window or GPU, then exits
  (non-zero on errors). After that the demo starts with everything ready.
  `build.rs` cannot cook: the build script has no VG builder/importer
  dependencies, so provisioning (download/convert) stays there and cooking runs in
* **Alpha-masked parts** are cooked like every other part (they are the
  largest by triangle count, e.g. fir variant a is 4.1M of which 4.07M masked;
  the per-file memory estimate already counts all triangles of the file).

### Instancing

One asset key is one mesh (kit) or one VG asset (one placeholder
mesh row, one `.vgeo`, one asset slot) however many placements use it; a
placement adds only an object row. The default scene (565 placements) has 31
distinct kit meshes for 331 kit instances and 16 distinct VG assets for 234 VG
instances. VG parts own no classic geometry: their mesh row is a one-triangle
zero-area placeholder whose interned vertex range is the key of the VG asset.
The kit's instances go through the object-batch pass (per-mesh batches). The VG
derivation re-publishes its buffers only when an instance is added or an asset's
readiness changes, not every frame. The startup log prints the live counts
(`[Museum instancing]`).

### Triangle selection and tau

`VG_STRESS_TAU=<px>` sets the VG error tolerance (default 1 px); `-` and `=`
lower and raise it by 20% / 25% live (0.1-64). The title shows tau, scene
triangles and VG selected triangles. Once the first pine (or first VG asset)
is ready the console prints the CPU reference cut at 20/50/100 m
(`[Museum LOD] ...`).

Architectural dimensions are calibrated against the kit manifest. Each scan is
displayed two metres tall; importer node transforms are included in the
calibration so unit conversion does not multiply the scene scale.

## Controls

WASD moves, Space/Shift moves vertically, left click grabs the mouse, and
Escape releases it (or exits when already released).

| Key | Debug view/action |
|---|---|
| `F3` | VG triangle ID |
| `F4` | VG cluster ID |
| `F5` | VG LOD level (cluster coarseness heat ramp) |
| `F6` | VG projected triangle size (pixels per triangle) |
| `F7` | VG raster tier (HW / micro-SW / dense / ray traced) |
| `F8` | VG page residency and streaming pressure |
| `F9` | VG projected error versus tau |
| `1`–`9` | Albedo, normals, roughness, metallic, AO, emissive, shadows, direct lighting, indirect lighting |
| `[` / `]`, Page Up / Page Down | Cycle every registered debug view |
| `0` | Normal rendering |
| `F1` | Print all available views to the console |

Pressing the active view's shortcut again returns to normal rendering. The
catalogue comes from the active render graph, including device-dependent
passes. The remaining Virtual Geometry views (detail vs root, software overdraw,
dense-tile overlap and mask, culling survival, instance ID, barycentrics, vis
normal, material bin, classic-vs-VG mask) are reached with `[` / `]`; the whole
category is registered by `helio-pass-vg-shade` under "Virtual Geometry".
VG views colour VG surfaces (the tile views cover the whole frame); classic
architecture continues through its regular debug path. `VG_DEBUG_VIEW=<id or
key>` (for example `lod`, `tier`, `101`) selects a view at startup; see
`helio_vg_format::debug_view` for the ids and keys. Views cost nothing while
unselected.

The title shows the active view, loading status, source scene triangle total,
and **VG selected** triangles. The scene total includes all loaded instances
and stays constant when moving. VG selected is a delayed asynchronous GPU
sample of emitted triangles across both culling phases. It excludes classic
architecture/preview triangles and shadow work; it is not a whole-frame count.

The title also carries the cull's stats line (selected triangles, emitted/tested
clusters, HW/SW/dense split, frustum/occlusion/backface/error culls and overflow
counters), read from the same asynchronous readback, so it adds no GPU stall.

Selection uses a screen-wide feedback controller (on by default) around the existing error-based hierarchy
cut. It compares the previous completed frame's selected triangles per viewport
pixel with `selection_triangles_per_pixel` (default 2) and adjusts the next
frame's `tau_px`. A denser cut raises tau so parent clusters replace complete
finer cuts; a lighter cut lowers tau to recover detail. It never rejects an
otherwise visible cluster to satisfy a count. Set
`HELIO_VG_SELECTION_TRIANGLES_PER_PIXEL` (0.25–32) for the target and
`HELIO_VG_SELECTION_SENSITIVITY` (0 disables the controller, default 1);
both can also be changed through `VgRuntimeSettings`. Set sensitivity to 0 to
disable adaptation. The controller (`helio_vg_format::density`: dead band, fast attack, slow release, demand normalised for the stats delay) steers to 0.7x the target. When enabled, effective tau stays between 1x and 8x the
authored setting; the count target cannot override quality indefinitely. If even the resident root cut exceeds the target, the
selected count stays above target rather than dropping geometry. The target is
a project tuning value; Epic's Nanite docs describe triangle count scaling with
pixel count but do not prescribe a fixed triangles-per-pixel ratio.

## Validation

```powershell
cargo test --release -p helio-vg-build --test build distant_sphere_has_bounded_density_and_sampled_surface_error -- --nocapture
cargo test --release -p helio-pass-vg-cull -p helio-pass-vg-shade
cargo test --release -p examples --features vg-stress --bin vg_stress
# Checks the cooked caches (run `cargo vg-stress -- --prepare` first):
cargo test --release -p examples --features vg-stress --bin vg_stress cached_museum_scans_reduce_triangles_with_distance -- --ignored --nocapture
```

The builder test samples actual simplified triangles against an analytic sphere
from three camera axes and requires at most 1 pixel of projected surface error
at 720p. It also checks monotonic triangle reductions and caps the 96m sphere at
1,024 triangles. GPU tests cover decreasing counts, restoring near detail,
instance-scale equivalence, both temporal phases, counter accuracy, and
off-screen rejection. Built-mesh tests check one cluster per leaf-to-root path.

The museum cache test measures the actual six Stanford scans with the demo's
corrected transforms, 45-degree field of view, 720p, and 1px error. It checks
monotonic distance reduction, a 25,000-triangle per-scan ceiling at 96m, a
7,000-triangle root ceiling, and eventual selection of the actual roots. It
also compares each cut with cone culling enabled and disabled, requiring the
cone-cull result to remain nonempty and never add triangles. These tests
validate CPU selection and projected error; the live GPU counters are still
needed to measure whole-scene frame time and total draw cost.

The same test also reports selected counts when the two-metre scan fills 18,
100, and 1,000 vertical pixels, for 1/2/4/8px geometric-error tolerances. The
current 1px results are denser than the default selection target:

| Scan | 18px | 100px | 1,000px |
|---|---:|---:|---:|
| Dragon | 9,322 | 23,744 | 233,340 |
| Happy Buddha | 6,536 | 25,706 | 144,518 |
| Manuscript | 6,276 | 6,276 | 79,268 |
| XYZRGB dragon | 9,719 | 43,383 | 1,180,339 |
| Thai statue | 6,432 | 37,796 | 313,766 |
| Lucy | 1,184 | 31,732 | 235,776 |

Those counts show that an error-only cut can be far denser than the target.
`sw_edge_px` only classifies selected clusters for rasterization; it does not
change the cut. The feedback now responds by choosing a coarser, still
coverage-complete hierarchy cut. The current cached scans also show cone culling
removes only about 0–2% from these views, so backface culling does not explain
or solve the main excess.

The texture timing printed at runtime spans the whole bounded producer phase,
including time the worker waits for the renderer to accept texture uploads; it
is not decode-only CPU time. Cold tree VG cache misses are prepared after
previews and texture events, so the scene is visible while this work continues,
but the loader can remain active well after first frame.

### Distance and backface fixes

The old caches used weighted quadric costs accumulated up the hierarchy. The
builder now simplifies with meshoptimizer while preserving material sections
and group boundaries, measures deviation against the source surface, carries
the maximum child error instead of summing it, and continues building while the
triangle count falls even if the cluster count stays level. It multiplies the
measured error by a safety margin. The cache cook revision invalidates output
from the older builder; cache replacement uses `std::fs::rename`, which replaces an existing destination on Windows and Unix.

The demo now enables conservative normal-cone backface culling on its opaque VG
instances. Alpha-tested tree cards remain classic meshes because VG does not
yet preserve their alpha mode; their sampled classic proxies are a remaining
source of distant geometry. Opaque tree trunks/branches and statue meshes use
their real VG geometry once the current cache is ready. A fresh run rebuilds
stale VG files automatically; the explicit scan cooker and distance test above
can be used to refresh and inspect the six museum caches offline.

For a hidden startup smoke run:

```powershell
$env:HELIO_VG_SMOKE_SECONDS = '18'
cargo vg-stress
Remove-Item Env:HELIO_VG_SMOKE_SECONDS
```

The prep tests (PLY ascii/binary parsing, normalisation, tar/gzip handling, GLB
writer, MD5, and a local HTTP fixture covering resume, retry, mirror fall-through
and hash mismatch) run with the same `cargo test` command and need no network.
The Poly Haven tests plan packages and texture sets from captured real API
responses (`vg_stress_prep/fixtures/`) and provision them end to end against a
local API stand-in (directory structure, manifests, credits, hash mismatch,
resolution fallback, recipe bump).
Changing the scan preparation recipe or the Stanford URLs lives in
`vg_stress_prep/scans.rs`; bump `RECIPE_VERSION` to regenerate GLBs.

## Foliage correctness and profiling

Leaf colour JPEGs require their separate linear PNG opacity maps. Provisioning
recipe 2 downloads these maps; the loader combines them before resizing and
preserves binary alpha coverage in the mip chain at the demo's 0.5 cutoff.
Missing opacity is a load error. Leaves use double-sided cutout materials, with
matching alpha tests in G-buffer, depth prepass, and cached shadow rendering.
Texture publication invalidates cached shadow faces. Opaque and two-sided
materials occupy separate GPU draw segments.

The classic Hi-Z test now projects conservative sphere intervals and covers
every queried mip texel. Camera, scene, draw-segment or resolution changes bypass
stale history until a matching depth frame exists. This single-phase fallback
can increase draws during motion; a current-depth second phase is still needed
for efficient occlusion during motion.

VG traversal redistributes its frontier after bounded local work, then drains
the last round. Exact-set GPU tests check 128 instances over 2/4/8 rounds and
1/256 workgroups, with no dropped or duplicate clusters. This changes work
scheduling, not the geometric error tolerance or selected triangles.

Set `HELIO_VG_PROFILE=1` to log interval frame percentiles, GPU pass timings and
readback age. With `HELIO_VG_SMOKE_SECONDS`, profiling keeps a fixed debug view
instead of cycling views. Run only one demo when comparing GPU times. The
corrected pixel-height table above uses `tan(FOV/2)`; the previous report used
`tan(FOV)` and mislabeled object sizes.

Remaining work: constant sampled leaf proxies are not canopy LODs, the cooked
trees should approach the density target at the authored base error before
runtime feedback has to raise tau, and texture cooking is not yet persistently
cached. Passing these regressions does not establish full-scene visual
stability or a production performance target.

## Tier 3 measurements (dense tiles)

Per-kernel GPU timestamps: with `HELIO_VG_PROFILE=1` (or capture mode) every
`VgDense` kernel is its own GPU scope (`vg_dense compact`, `make_args`,
`bin (count|scan|fill)`, `peel`, `peel masked`, `collect`, `mask_union`,
`footprint_args`, `footprint`, `vg_dense_rt compact|args|trace`). They appear in
`gpu_ms` of every capture stats JSON; with profiling off nothing is recorded. The
stats JSON also carries `vg.tier3` (frames per backend, dense tiles, list sizes,
TLAS rebuilds/reuses, CPU microseconds of the RT preparation) and
`tile_density` (distribution of the Tier 2 per-tile triangle / hit counters, read
back once per shot).

Shots: `tier3-off` (ForceHwRaster), `tier3-tier2` (Auto, Tier 3 disabled: the
baseline for Tier 3), `tier3-on-peel` (ForceNonRtFallback), `tier3-on-rt` (Auto
with ray queries), plus `-dense-overlap` views. Run one density at a time:

```powershell
$env:VG_STRESS_DENSITY = '3'; $env:VG_STRESS_PAGE_MB = '2048'
$env:VG_STRESS_CAPTURE = 'out'; $env:VG_STRESS_CAPTURE_ONLY = 'tier3-tier2,tier3-on-rt'
$env:HELIO_VG_PROFILE = '1'; cargo vg-stress
```

Tier 3 is opt-in for this demo: set `VG_STRESS_TIER3=1` to run its ray-traced
dense-tile backend when hardware and proxy residency allow it. The “Ray Trace vs
Raster” debug view only shows which path produced each pixel; it does not enable
Tier 3.

In RT mode, the resolved dense-tile mask replaces raster visibility for those
tiles. Culling diverts clusters wholly contained by the mask out of the raster
bins; clusters crossing tile boundaries remain rasterized outside the mask.
Candidate collection includes HW, SW, and dense bins so the TLAS covers all
visible opaque VG geometry touching the tiles. Candidate readback and TLAS
limits are currently 16,384 instances; overflow or incomplete proxy coverage
falls back to the non-RT dense path. Alpha-masked materials are not represented
by RT proxies yet; replacement mode currently resolves the proxy's geometric
surface without its alpha texture.

Calibration overrides (read when the raster is created): `HELIO_VG_DENSE_TRI`
(base triangles per tile, default 64), `HELIO_VG_DENSE_HIT` (base depth-passing
hits per tile, default 64), and `HELIO_VG_DENSE_SENSITIVITY` (default 1, clamped
to 0.125–8). Higher sensitivity divides both dense thresholds, so tiles qualify
sooner; for example, `2` uses thresholds 32 triangles and 32 hits. The raw
threshold overrides are scaled by sensitivity too. `HELIO_VG_DENSE_LATCH` sets
how many frames a tile stays dense (default 8), and
`HELIO_VG_DENSE_MAX_TILES` caps qualifying tiles per frame (default 512).
`VG_STRESS_PAGE_MB` sets the page-pool budget.

```powershell
$env:HELIO_VG_DENSE_SENSITIVITY = '4'
$env:VG_STRESS_TIER3 = '1'
cargo vg-stress
```

### What the 93 ms was (forest shot, RTX 4090, 1600x900, density 1)

| VgDense kernel (ms) | before | after, depth peel | after, ray traced |
|---|---|---|---|
| `peel` (opaque) | 31.64 | 0.79 | - |
| `peel masked` | 62.80 | - (masked clusters stay on Tier 2) | - |
| `bin` count / scan / fill | 0.14 / 0.06 / 0.31 | 0.012 / 0.045 / 0.015 | - |
| `compact`, `make_args` | ~0.01 | 0.025, 0.007 | 0.008, 0.007 |
| `mask_union`, `footprint(_args)` | ~0.03 | 0.007, 0.031, 0.007 | 0.006, 0.010, 0.006 |
| `collect` | 0.054 | - | 0.012 |
| `vg_dense_rt compact/args/trace` | - | - | 0.008 / 0.006 / 0.043 |
| **VgDense total** | **95.0** | **0.96** | **0.15** |

Root cause: not the binning, not atomics, not the readback. Tier 3 *replaced*
every Tier 1/2 pixel inside a dense tile, so each tile's list held every cluster
whose bounding sphere touched it (about 2,500 per tile, 8,265 at most; 478k list
entries over 193 tiles) although only about 41 triangles per tile (opaque) and
3,200 (masked) were real. A tile is one workgroup that re-streams its whole
cluster list once per peel layer, so the pass lasted as long as the slowest
tile (about 30 ms opaque, 60 ms for the masked pipeline, which also re-scanned
the list serially). The fix is structural: Tier 3 now *adds* only the clusters
the cull diverted to the dense bin (about 8 per tile) and `vis_resolve` takes the
nearest of that and the Tier 1/2 pixel (`FLAG_DENSE_MERGE`), so alpha-masked
leaves never enter Tier 3.

### Does Tier 3 beat Tier 2? (measured)

Tier 2 only (`tier3-tier2`), contended GPU (another process was using the card
during these runs, so absolute times are about 2x the quiet ones; compare within a
row): density 1: 12.6 ms for 6.2 M selected triangles, density 3: 26.7 ms for
23.6 M, density 8: 40.3 ms for 40.2 M. The Tier 2 per-tile counters never exceed
2,386 triangles (9.3 per pixel) or 429 depth-passing hits (1.7 per pixel) per tile,
p99 1,257 triangles. Tier 2 resolves that at about 3 G triangles/s: a 2,386-triangle
tile costs about 0.75 us of whole-GPU time. The depth peel costs about 4 us per
tile on the diverted clusters alone (0.79 ms for 193 tiles on a quiet GPU), the ray
trace about 0.2 us per tile plus 0.1 ms fixed. There is no crossover for the peel at
any measured density on this GPU (Tier 2 uses 64-bit atomics, so overlap costs it
nothing extra); the ray trace only breaks even above roughly 2.5 triangles per pixel
across 100+ tiles, and at density 3 or more it never became ready with the default
512 MiB pool because the cull's own demand holds every page slot (proxy pages are
the lowest priority and the pool will not evict recently used pages). With a 2 GiB
pool it did, and was not faster than Tier 2 (28.9 vs 25.9 ms contended). Hence the
policy: Tier 3 stays opt-in; in `Auto` the peel runs only where Tier 2 is the tiled
no-int64 fallback (its lists overflow), and the ray trace only when it is ready.
Enabled but with nothing dense, `VgDense` costs 0.016 ms (a probe every 4th frame).

`tier3-on-rt` reports `frames_rt` in the hundreds; RT and depth-peel images were
bit-identical on the forest shot and differ from Tier 2 only on 0.0135% of pixels.

## Instance-scale benchmark (`VG_STRESS_INSTANCES`)

N instances of ONE asset (default the Thai statue, `VG_STRESS_BENCH_ASSET=thai|lucy|pine`)
in a seeded jittered grid with varied yaw and scale (spacing `VG_STRESS_BENCH_SPACING`,
default 3.2 m) over a classic ground plane, nothing else. The scene is built by the normal
loader from the cooked caches (run `cargo vg-stress -- --prepare` once); textures are skipped.
One process per N; it renders six fixed camera cases offscreen (no window) and exits. Tier 3
stays off. A private-memory guard stops the process at `VG_STRESS_BENCH_MEM_GB` (default 14).

```powershell
cd D:\GitHub\Pulsar-Native\crates\renderer\helio
$env:HELIO_VG_STRESS_SKIP_PROVISION = '1'
# Close a running vg_stress first (Windows locks the exe), or build into another target dir:
#   $env:CARGO_TARGET_DIR = 'D:\GitHub\helio_bench_target'
cargo build -p examples --features vg-stress --release --bin vg_stress

foreach ($n in 1000, 5000, 20000, 100000) {
  $env:VG_STRESS_INSTANCES = "$n"     # or: vg_stress.exe --instance-bench $n
  $env:VG_STRESS_TREES = 'none'
  $env:VG_STRESS_CAPTURE = "out\bench_$n"
  $env:VG_STRESS_CAPTURE_RES = '1600x900'
  $env:HELIO_VG_PROFILE = '1'         # per-pass GPU timestamps
  .\target\release\vg_stress.exe      # (or $env:CARGO_TARGET_DIR\release\vg_stress.exe)
}
# A/B of the root early-out:  $env:HELIO_VG_NO_ROOT_SHORTCUT = '1'
# Classic instancing check:   $env:VG_STRESS_BENCH_CLASSIC = '20000'   (20k quads, one mesh + material)
# Subset of cases:            $env:VG_STRESS_CAPTURE_ONLY = 'bench-far,bench-away'
```

Output: `out\bench_<N>\bench-<case>.json` (+ a small `.jpg` per case and `index.json`). Cases:
`bench-far` (whole grid in view), `bench-away` (standing at the grid edge looking away),
`bench-edge` (edge, looking along the grid), `bench-inside`, and `bench-far-move` /
`bench-inside-move` (1% of the instances move 2 cm every frame, a rotating window). Keys of
interest: `gpu_ms` (per pass, mean of the last 30 frames, `TOTAL`), `vg` (`selected_triangles`,
`clusters_tested/emitted`, `jobs`, `root_shortcuts`, `culled`, `overflow.{jobs,visible,peak_queue}`,
`pool`), and `bench`: `spawn` (ms, us/instance, private bytes per instance), `cpu_frame_ms` /
`wall_frame_ms` (min/mean/p50/p95/max over the last 40 frames), `bvh_frames` (frames in
skip / refit / rebuild during the shot), `derivation` (calls, rebuilds), `memory`, `gpu_bytes`
(`static_objects` rows x 252 B, derivation buffers, BVH scratch, per instance).

### Results (RTX 4090, 1600x900, Thai statue, Tier 3 off; GPU CONTENDED)

Another vg_stress process (the user's own interactive window) held the GPU at 97-99% for the
whole session, so `TOTAL` swings by several ms between runs (Atmosphere, VolumetricFog and Lens
varied 10x with no VG change). Compare the VG pass sums and counters, which are deterministic;
absolute times are upper bounds. Sum of the `Vg*` passes in ms, before -> after the changes below:

| N | far | away | edge | inside | far, 1% moving | inside, 1% moving |
|---|---|---|---|---|---|---|
| 1k | 2.9 -> 2.8 | 0.36 -> 0.23 | 10.6 -> 3.4 | 7.4 -> 3.1 | 6.0 -> 1.9 | 4.7 -> 3.2 |
| 5k | 1.1 -> 2.3 | 0.22 -> 0.28 | 7.4 -> 5.0 | 6.4 -> 4.8 | 8.0 -> 1.7 | 10.5 -> 4.6 |
| 20k | 4.4 -> 2.2 | 0.22 -> 0.23 | 6.7 -> 5.9 | 6.0 -> 5.6 | 7.1 -> 2.8 | 9.3 -> 4.7 |
| 100k | 5.5 -> 7.9 (*) | 2.3 -> 0.85 | 9.3 -> 12.5 (*) | 6.0 -> 6.4 | 17.6 -> 7.2 | 18.6 -> 6.8 |

(*) After the change the 100k far view draws 999,510 clusters (112 M selected triangles); before it
silently dropped 475,222 of the 1,000,000 needed clusters (bin ceiling), so "before" did half the
work. 500k: far view needs 5.0 M clusters, 1,048,576 are kept and 3.9 M are dropped (counted).

Counters (deterministic), 100k far, root early-out off -> on: jobs 3,398,441 -> 299,912; VgCullEarly
2.13 -> 1.18 ms measured back to back (`off_100000` vs `on2_100000`); `root_shortcuts` 99,963 of
100,000 instances. 1% moving, 100k: `VgBvh` 10.98 -> 0.09 ms (refit instead of rebuild every frame),
`ObjectBatch` 8.53 -> 0.25 ms. Static steady state at every N: BVH skip on 69 of 70 frames
(the first frame builds), derivation rebuilds 0.

| N | spawn (all rows) | us / instance | CPU bytes / instance | GPU bytes / instance (252 B row + 16 B instance + BVH scratch) |
|---|---|---|---|---|
| 1k | 0.9 ms | 0.88 | (below page granularity) | 469 |
| 5k | 4.0 ms | 0.81 | 1,225 | 593 |
| 20k | 16.9 ms | 0.84 | 965 | 593 |
| 100k | 78 ms | 0.78 | 732 | 525 |
| 500k | 380 ms | 0.76 | 578 | 470 |

Process private memory: 2.8 GB (1k) to 4.3 GB (500k) including ~1.2 GB of runtime/driver baseline. CPU
frame time (render call) stays at 2-4 ms for every N once the per-frame `asset_ready_rows(N)`
padding of the example derivation was removed (it cost 27-45 ms per frame at 500k).
Classic instancing (`VG_STRESS_BENCH_CLASSIC=20000`, 5k VG instances): the 20k quads draw through the
object batch (GBuffer 0.034 ms, ObjectBatch 0.002 ms), VG instances still draw, classic rows are not
suppressed.

### Limits that remain

* A far instance costs its root clusters, not one: Thai's roots are 10 clusters (1.1 k triangles), so
  100k distant instances are 1 M clusters / 112 M triangles in the micro-raster. Cost then follows the
  instance count, not the pixel count; fixing it needs a coarser top level from the builder (see
  plan VG-BUG-6), or an instance-level sub-pixel cull.
* Hard caps degrade by dropping clusters: one bin holds at most a quarter of `max_visible_clusters`
  (1,048,576 at the default, bounded by 8 bins x 2^20 < the 2^24 visible index and the 25-bit vis id).
  Drops are counted in `vg.overflow.visible` and in the window-title stats line (`overflow N`); there
  is no debug view for them.

### Foliage rollback (cook revision 7)

Revision 7 restores the revision 5 foliage builder. Revision 6 grew surviving
connected pieces using a material/group-wide area ratio, which did not preserve
local projected coverage. It also imposed geometric distance error on sparse
foliage, making LOD selection retain much finer cuts. Its section-wide rejection
rules could prevent simplification altogether. These changes are reverted.

Run `cargo vg-stress -- --prepare` to rebuild stale caches, including revision 6.
Triangle-density adaptation remains opt-in and bounded; compare selection at a
fixed tau with adaptation disabled when evaluating future builder changes.

The synthetic 63,200-triangle canopy test isolates the selection regression at
240 units and fixed tau 1: revision 5 selects 15,763 triangles; revision 6 selects
62,090; adding only the geometric-distance error selects 62,244; adding only
area compensation selects 16,861. The geometric-distance floor is therefore
the dominant selection regression in this fixture. These are CPU selection
measurements, not museum GPU timings or visual validation. The existing test
now requires substantial reduction at this distance, where its older assertions
allowed revision 6 to pass. Future coverage work needs local projected coverage
and silhouette comparisons before changing the selection metric.

### Cook revisions 8 to 10: foliage coverage and the density bound

Revision 8 replaced the proximity merge of floating pieces (needle and leaf
cards) with an area-preserving card merge: nearby cards are merged by recursive
bisection into one quad whose area is the sum of its members' (coverage is kept),
whose length is the members' mean length (scale is kept) and whose width is the
only thing that grows, and cells that straddle a gap are split instead of merged.
The merged group keeps the measured deviation, so the cut selects it only where
that error is below the tolerance. Group partitioning also gained a spatial
fallback: clusters that share no edges (disconnected foliage) are grouped with the
nearest free cluster in a Morton window rather than the next one in curve order,
which had been stringing far-apart clusters into one group and pinning the error
of every ancestor.

Revisions 9 and 10 bound selected triangles per pixel in the stored errors. A
group whose children are over `DENSITY_TARGET` (1.5) triangles per pixel for a
whole-asset level (level triangles over the mean silhouette area of the asset) may
be selected before its measured error reaches the tolerance, never below
`measured / 12`. Geometry is never dropped: the coarser level stands in for all of it.

Validate with `cargo test --release -p helio-vg-build --test lod_validation --
--nocapture --include-ignored` (set `VG_LOD_OUT` for images). Real caches:
`VG_CACHE_DIR=<cache dir> VG_REBUILD=1 ... real_cached_assets_ladder_and_images`.

### Computed screen-wide density (cook revision 11, format version 4)

The feedback controller described above is gone. Every asset header now carries
its bounding sphere and a 32-entry `LodTable` (selected triangles against
object-space error tolerance, written by the builder). Before traversal the cull
pass runs `cs_density` (one workgroup): each frustum-visible instance looks its
asset's table up at the tolerance its centre distance implies, for 13 candidate
multipliers (1x to 8x); the smallest multiplier that fits
`0.75 * selection_triangles_per_pixel * pixels` is written to the state buffer
and used by `load_inst` for every cut that frame, rate limited to x1.25 up and
x0.96 down per frame. Only the occlusion share is learned (CPU, from the late
stats: `VisibleFraction`), and it only scales the budget. CPU mirror and tests:
`helio_vg_format::density`; benchmark: `forest_density_is_computed_screen_wide`
(demo trees, ~2,100 instances: worst 1.92 triangles/pixel, mean 1.25, prediction
within x1.29, per-frame multiplier change <= x1.16). Merged foliage cards now take
all four corners' attributes from one member so their texture stays coherent.
