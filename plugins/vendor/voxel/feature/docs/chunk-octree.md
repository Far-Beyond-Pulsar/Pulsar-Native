# Chunk octree: the voxel terrain renderer

The terrain is drawn from **chunk trees**: 64³-cell chunks generated on the
GPU from the terrain contract, kept in an octree per cube face whose cut
rays decide, and rasterized as boxes that rays enter only inside one chunk.
Code: `src/chunks/` (`mod.rs`: `ChunkWorld`; `classify.rs`, `residency.rs`,
`drawn.rs`, `pass.rs`, `scene.rs`), `shaders/chunk_generate.wgsl`,
`shaders/chunk_draw.wgsl`, `shaders/chunk_table.wgsl`.

## Volumes

A scene holds any number of voxel worlds (`crate::volume`): a
`VoxelVolume` is a world (its recipe and edits, a `Planet`) and an f64
`VolumeTransform`, a rotation then a translation that may change every
frame. A planet, a moon, a plane and a ship-sized block are all volumes.
Every world keeps its own frame (planet-centred, or the plane's): its
cells, edits, CPU queries and chunks never change as it moves.

One `ChunkWorld` holds them all (`ChunkWorld::add_volume`,
`set_transform`, `remove_volume`; `ChunkPass` follows a frame's
`PlanetFrame::volumes` by their host ids):

- **One chunk pool.** A node key carries its volume (`NodeKey::volume`,
  bits 16 and up of the chunk table's key x), so slots, leaf and detail
  pages, the chunk table, the drawn cut, kept chunks and the readbacks are
  shared; eviction under pressure takes each volume's share by the chunks
  it holds.
- **One planner.** Its thread plans every volume a job (`residency::
  Worker`), each with the eye, view and velocity in its own frame: the eye's
  own motion and the volume's under it (a moon's orbit), so residency
  follows screen size in every volume alike.
- **Generation per world.** A volume has its terrain program's pipelines
  (shared by volumes of one program and form), its own footprint pool (no
  larger than the footprints it has: a block's is a few slots), features,
  brushes and baked bricks. One volume generates a frame: those with edits
  first, then those with requests, in turn.
- **One tracer, a pass a volume.** A view traces its volumes nearest first
  (by their bounds; the primary one always), each by the same kernels with
  its own `DrawParams`: the eye in its frame, the camera's projections
  composed with its rotation, the sun and the scene's lights turned into
  it (`to_local`), the last frame's projection from where its transform
  had its points (moving volumes write their own motion vectors). A later
  volume's seed keeps only boxes nearer than the hits already found, and
  its trace walks no farther than them, so the nearest hit of any volume
  wins per pixel, hidden volumes cost little, and volumes may overlap (a
  ship landed in the ground). Every pass reads the meshes' depth the same
  way (Drawing 1, 2): meshes hide every volume; only the first pass keeps a
  hidden pixel's frame chunk (its pass walks the meshes' points). A chunk's frame stays in its volume's
  coordinates: a moving or spinning volume rewrites none. A later pass's
  kernels run only over its volume's rectangle on the screen
  (`DrawParams::rect`: the projected box of its bounding ball, origin on
  32 pixels; the whole view when the box reaches behind the eye), so a
  volume costs by the pixels it may cover, as rasterised bodies do.
- **Shading per volume.** Every shading kernel runs a dispatch a volume
  and takes the hits of its pass's volume (`ours`; the pixels no volume
  hits, the first pass's): sun, sky and local lights walk the hit's own
  volume (a volume shadows itself exactly). Normals leave in the world's
  axes (`to_world`); the denoiser runs once in the world's frame,
  reprojecting each hit by its volume's motion (`motions`).
- **Lighting across volumes.** Any ray can walk any volume from where it
  enters it (`enter_volume`: a sphere's ceiling ball, a plane's band
  between its edges; placed by the volume's anchor chunk, else a sphere's
  exact levels). Each volume that can matter adds a pass in its own
  pipelines (`ChunkWorld::cross_volumes`): its sun shadow on the others'
  hits and the meshes, ANDed in, over the rectangle the shadow can fall in
  (`shadow_cross`; pixels the sun's occlusion pages lit are left: every
  volume is traced into them); the GI probes' rays, the nearer hit kept
  (`gi_trace_cross`); foliage's probes, where it stands on the ground
  found or holds it (`ground_probe_cross`). Single-volume worlds run and
  build none of them.
- **The primary volume** is the one whose ground the eye is nearest
  (`follow_eye`): foliage is tiled on it and GI probes are placed in its
  cells.

CPU queries take world positions: `Volumes` (exact casts and solid tests
over every volume a ray or point meets, nearest hit first) and
`VolumeQueries` (each volume's `WorldQueries` bricks). Picks name the
volume they hit (`PickHit::volume`); their cells are in its frame.

## Nodes and chunks

- A **node** is a chunk: `(face, level, cx, cy, cz)`, 64³ cells of its
  level (6.4 m at level 0 and 0.1 m voxels). Its 8 children are
  `(level - 1, 2cx + a, 2cy + b, 2cz + c)`: cells halve in all three axes,
  so layers nest. Planes use one face; an infinite plane is a face 2^27
  reference cells across.
- A **footprint** `(face, level, cx, cy)` is the 64×64 columns over a stack
  of nodes: the unit of column generation (heights, surface words, volume
  extents) and of classification.
- **Roots** are every node of the coarsest level from the planet's centre
  (a plane's lowest ground) to the ceiling, and as deep and high as the
  edits reach. Residency does not depend on where the eye started.

A chunk's contents are a **64-tree**: a root over 4³ mid nodes of 16³
cells, mid nodes over 4³ leaves of 4³ cells, a leaf a 64-bit occupancy
mask. Bit `x + 4 y + 16 z` of a mask is child or cell `(x, y, z)`. Children
are packed and addressed by popcount rank; full children are flagged in a
second mask and store nothing. Leaves live in pages of 64 (up to 64 pages a
chunk) off a free stack on the GPU, so freed storage serves any chunk; an
evicted chunk's pages go back (`evict`).

Per stored leaf, two material bytes (its **pair**: the surface material,
shown by cells under air, and the subsurface material, shown by the
others) and its ground slope (two bytes, by angle). A full leaf is
implicit (drawn with its mid's material) unless a column tops in it or its
pair would draw cells wrong. Per-voxel materials are under
[Materials](#materials).

Each chunk has an **affine frame**: world = origin + M cell, the grid
mapping's central differences across the chunk about its columns' mean
top. Origins are f64 on the CPU, uploaded as a high and a low f32 and
rebased to the eye on the GPU, so every position is small at any planet
size. Generation also fits a plane to the chunk's column tops (least
squares, lifted over every top, plus a sixty-fourth of a cell;
`ChunkMeta::top`): its raster box is tilted to that plane, so a ray over a
slope is seeded at the chunk it hits. A plane rising past the chunk's cube
would reach into the chunk above; such a chunk keeps the flat top of its
occupied layers, so boxes never overlap.

### Exact cells

Across a chunk the sphere curves `512 x cell / radius` of a cell from the
frame. From the level where that passes a twentieth of a cell
(`chunks::exact_level`: 13 on an Earth at 0.1 m voxels, 820 m cells; none
on planes) chunks and air are walked in the grid's own cells: on a face a
cell's sides are planes through the planet's centre and spheres about it,
so a ray's stretch in a box of cells is four half-lines and two quadratics
(`box_span`; leaving one, only the sides ahead: `box_exit`). Rays are
watertight across chunks and faces from orbit, shading reads the grid's
axes at the hit (`cell_frame`), and an exact chunk's raster box grows by
how far its cells reach past its frame's (`reach_past`).

The exact code costs registers in every walk compiled with it. Kernels for
hits finer than the exact levels (sun and sky samples) use `walk_fine`,
generated from the same source without it (`walk_templates`); their `_far`
variants take the exact hits; primary rays walk both.

## Classification

A node is one of (`classify::Kind`):

- **Mixed**: a surface may cross it; generated into a chunk slot.
- **Empty** or **Solid**: uniform; drawn without data (air to cross, rock
  to hit).
- **Buried**: within the field's volume (caves may lie there) but under
  the surface band; drawn as rock until a ray hits it or it holds the eye,
  then generated like a Mixed node.

`EditView::classify` decides a footprint's nodes from its ranges (the
field's volume and surface band, measured on the GPU or estimated from the
parent's quarter of columns over it, widened by a parent cell; the
parent's whole ranges while it is estimated too) and the edits over it: the field's
surface band and every edit's are Mixed (merged where fewer than two
chunks apart), the rest of the field's volume Buried, the rest uniform.
Outside every band a node lies wholly inside or outside each brush:
`EditView::kind` takes the last brush that adds or removes at its middle,
else the field (rock under its volume, air over it).

A node answers for its descendants, whose ground its own band-limited
cells may not show:

- **Rounding.** Coarse cells round their tops down. The field's band is
  widened by `DESCENT` (2) cells, and a Mixed node whose chunk comes back
  empty is still refined (drawn as air).
- **Caves.** A column's volume extent is empty where a level cannot show
  a cave ([`volumes.md`](volumes.md)); the Buried band reaches
  `Planet::volume_bounds` under the surface at every level
  (`EditView::volume_depth`).
- **Small edits.** A brush under half a cell is inactive at a level, and
  coarse baked bricks are built by majority (a 4 m shaft vanishes in 6.4 m
  cells). Brushes classify at any size; `BrickIndex` marks every brick's
  node and all its ancestors.
- **Brush bands.** A box's band is its heights; a ball's, the heights of
  its sphere's lower and upper caps over the footprint
  (`EditView::surface_bands`), so a large blast asks for the layers its
  surface crosses, not its whole height.

## Residency

`residency::Planner` runs on a worker thread (`Worker`) and keeps the cut
between plans ([Incremental residency](#incremental-residency)).
`ChunkWorld::update` sends it the eye, velocity, view and what happened
since (`Event`) and takes the latest plan without waiting: its drawn set as
differences from the last, and its requests.

- **Refinement.** A Mixed node is refined while its cells project larger
  than `lod_pixels` from the eye or from where it will be in
  `prefetch_seconds` (3D distance to its band's box), until they fall under
  `coarsen` of that. Allowances raise the target: `offscreen_pixels` out of
  view (past the view cone widened by `view_guard`), `unseen_pixels` where
  no ray met it for `seen_frames`, up to `motion_max` where the eye's
  motion sweeps the image. On a sphere, nodes below the horizon over the
  lowest ground are drawn as they are; a plane stops at `max_distance`.
  Empty, Solid and Buried nodes are never refined.
- **A band switches as one.** A node's band is its footprint's contiguous
  Mixed layers, and every test (distance, view, rays, horizon, readiness)
  is the band's: a coarse cell's ground, rounded, may lie in the node above
  or below. Separate bands (caves, digs) switch apart.
- **Rays decide where detail goes.** Ray reports mark the node a hit wants
  and its ancestors seen (see [Ray reports](#ray-reports)). Rays only keep
  nodes from refining: coarsening a refined node no ray met oscillated at
  occlusion edges. Reports are not structural events, so they never keep a
  plan from settling.
- **Readiness.** A refined node is drawn as its children once every Mixed
  child is generated; until then it is drawn itself, so nothing is ever
  missing. Children are classified from the parent footprint's ranges; a
  footprint measured later reclassifies its nodes. Buried nodes never hold
  a parent back.
- **Buried nodes** are generated when they hold the eye (`near_cells`) or
  a ray met them.
- **Eviction.** Generated nodes out of the cut for `release_frames` (600
  by default, `HELIO_VOXEL_CHUNKS_RELEASE_FRAMES`) are released,
  `MAX_RELEASE` a plan, longest out first, so residency follows the cut,
  not the eye's history. When chunk slots or leaf pages run short (an
  eighth of the pool, at least two batches' worth), the pool grows; at its
  most (`Capacity::chunks`), those out of the cut longest go first (never
  one in it in the last `IDLE_FRAMES`: it may still be fading out). Nodes
  found empty are remembered as long as chunks (a cut that comes back does
  not generate them again); bookkeeping of nodes never generated is dropped
  after `keep_frames`.
- **Pools grow on demand.** The slot pool (8192 slots at first: meta, mids,
  keys, statics, leaf page lists, draw lists) and the leaf pool (leaves,
  their materials and normals, each page's detail page, the free stack)
  double when short, up to `Capacity::chunks` (131072 by default): every
  buffer is copied into one twice the size in a submit of its own, before
  the update records anything, so writes queued after land in the new
  buffers. The chunk table grows with the slots (rebuilt from its CPU
  mirror and uploaded whole); the leaf stack takes the new pages under the
  old ones and its count rises on the GPU (`chunk_pool.wgsl`: the count
  read back lags it, and readbacks from before a growth are ignored). A
  settled Earth view at 60 m holds a 65536-slot pool: the chunk buffers
  take 1.5 GB of GPU memory, not the 1.9 GB of the most.
- **Settled.** A plan counts only for the eye and view it was made for, and
  only once it saw a sweep of ray reports of the cut as it ended up: after
  residency last changed and the last fade ended (what a finer node
  uncovers shows once its fade ends), past the reports in flight then
  (`REPORT_FRAMES`, 8). Hosts render until then.

### Incremental residency

A plan's cost follows what changed, not the tree: the cut is kept between
plans, and a plan visits only nodes whose outcome may have changed, coarsest
first (a node coarsening takes its subtree out of the cut before its nodes
would be visited; one refining visits its children as they enter).

- **Re-checks.** Each visit bounds how far the eye may move, its
  velocity change and the view turn before its outcome could change, and
  queues its next visit on three odometers (metres the eye has travelled,
  metres a second its velocity has changed, radians the view has turned);
  a window lets the velocity change by what moves the prefetch eye a
  quarter of the eye's travel more. The bounds are taken over the whole
  window, halving it from twice the distance until no threshold can be
  crossed: distances to a box move `DRIFT` (1.25) times as fast as the eye
  (the linear frame drifts with it), the motion allowance by the velocity
  and the offset's turn, cone margins `(1 + |cos|) DRIFT` times (and a
  box's breadth a radian with the view), the horizon by `e / horizon` a
  metre of climb; a box measured near the eye switches to its centre where
  it meets the frame's reach. A band far too coarse keeps its chain of
  wanted nodes under the eye while each step's nearest child stays the
  nearest and its error stays on its side of 1. A threshold may be passed
  by `LATENESS` (1/16 of its measure) before the visit, so a node exactly
  at one is not visited every plan. Ray reports' expiry (`seen_frames`)
  and edits' waits queue on frames. Each node keeps the due reading of
  each queue and is queued again only when that comes earlier (an entry
  that comes out before its due is queued again at it).
- **Events** mark the nodes they touch, their footprints' other nodes (a
  band, rock waiting for its surface) and their parents (readiness); a ray
  report marks a node only when rays had not met it lately. Nodes that
  want others generated are visited every plan: their requests'
  priorities follow the eye. A band whose members stay open and that no
  event marked keeps its children as they were (its children's own visits
  want what they need). A view whose shape changes, or the first ray
  report, visits the whole cut once.
- **Eviction never looks at the cut.** Nodes leaving the cut, or touched
  while out of it, queue by idle frame in a heap per kind (chunks, empty,
  never generated); eviction pops the longest idle, skipping entries of
  nodes that came back or changed since.
- **The plan's measures** (`coarse_share`, `worst_px`) are taken over the
  leaves in view as their last visit measured them: within `LATENESS` of a
  visit now for the thresholds, and a leaf's cells as they were then.
- **Exactness.** `incremental_plans_match_full_visits` checks, with no
  lateness, that every plan of a flight over a plane and an Earth (cruising,
  climbing, turning, rays reporting then expiring, a request budget) draws
  and wants exactly what a visit of the whole cut does.

## Generation

`ChunkWorld::update` generates a frame's batch within a GPU budget
(`ResidencyConfig::generate_ms`, 3 ms; `ChunkPass` lets it grow into what
the last frame left of `frame_ms`, up to three times) against the measured
cost of a footprint's columns and of a chunk (`GenCost`), at most
`footprints_per_frame` footprints. Nodes edits changed go first (coarsest,
then drawn ones nearest first), then the plan's requests, a footprint's
nodes together; kept chunks come back by copy.

A footprint's columns depend only on the terrain, so they stay on the GPU
in the **footprint pool** (a volume's: 1024 slots at first, doubling up to
2048 when it would reuse a slot used in the last 120 frames; about 75 KB
each, least recently used reused, never one the batch uses): a footprint is
computed once while it stays in use, whichever of its nodes are generated
when. The two-rate tile nodes are the batch's scratch, one buffer for the
volumes' largest program. The batch
lists the footprints it computes first, then those it reads from the pool.

Kernels, a timed pass each (`FrameTimings::stages`, `tile_stages`):

1. `pack_main`, `evict`: edited chunks being evicted are packed (see
   [Edits](#edits)); evicted chunks' leaf and detail pages go back on the
   free stacks.
2. `tile_nodes_main`: the two-rate field's nodes of each computed footprint
   ([`two-rate-field.md`](two-rate-field.md)).
3. `tile_slope_nodes_main`, `tile_columns_main`, `tile_slope_heights_main`,
   `tile_slopes_main`: each column's height, surface word and volume
   extent, and the footprint's measured ranges by quarter (each a child
   footprint's: a coarse footprint's whole range spans all its children's
   relief); for levels 0 to 3 the
   level-4 heights and block slopes materials read
   (`Planet::material_slope`), kept with the columns in the pool.
4. `tile_caves_main`, `tile_volumes_main`: the footprint's cave
   primitives from its regions' networks, then each column's volume
   extent (cliff band, caves over it) and the measured volume range
   ([`volumes.md`](volumes.md)).
5. `tile_features_main`: the footprint's feature placements
   ([`features.md`](features.md)), its ranges raised over them.
6. `chunk_columns_main`: each column's 64 cells in a chunk as a 64-bit mask
   (bit z: layer `64 cz + z`): the heightfield, cliff shapes and caves,
   features, then the edits in order (large brushes, baked cells, recent
   brushes), and the materials of its cells. A workgroup (8x8 columns)
   culls the feature placements over it and their parts together, a lane
   a square or part, so each column tests only those that can meet it.
7. `chunk_cells_main` (chunks with per-voxel materials, below): a lane a
   column, its exposed cells' materials; `chunk_build_main`: the chunk's
   64-tree, leaf pairs and slopes, detail pages, top plane and frame
   height, into leaf pages off the free stack.
8. `restore_main`: kept chunks copied back in.

Results come back a frame later without waiting (a readback ring):
measured footprint ranges reclassify their nodes, empty chunks free their
slot, and each chunk's occupied layers and mean top give its frame.

## Edits

A `ChunkWorld` follows each volume's published planet edits by difference
(`ChunkWorld::sync_edits`), by the history's hash; each volume keeps its
own edit view, brushes, brick cache and urgent nodes, and its planner its
stand-ins:

- Brushes that came or went mark the nodes their bands cross at every
  level they apply at: drawn and visited ones regenerate (urgent), cached
  ones not in the cut are evicted. Urgent nodes go drawn in view first,
  nearest first (up to `EDITED_NOW` of them past the frame's budget, so a
  small dig shows the frame it is made), then drawn ones out of view, then
  the cut's ancestors.
- **What shows meanwhile.** An edited node keeps drawing its old chunk
  until its new one is generated; a node an edit made Mixed (a crater's
  floor, a mound over the ground) is drawn as it was (rock or air: the
  planner's `stand_in`, handed down to its children) without holding its
  parent back; one too coarse for the eye in view hands that to its
  children instead, so the edit is generated at the level the eye wants.
  Rock an edit added where there was air (a mound's inside) stays air
  while its footprint's new surface is generated (`ADDED_ROCK_WAITS`). So
  an edit never shows a coarse ancestor in its place (it once drew flat
  polygons hundreds of metres across for seconds after a big stroke).
  Rock a ray meets (Solid and Buried nodes) draws the brush that filled
  it or the volume's rock (`Planet::rock_material`, in the table entry):
  it drew material 0, magenta, where an edit uncovered it.
- Changed baked columns regenerate the nodes over them within the budget.
- Sync is incremental: the large layer's faces live in shared runs
  (`Faces`) and their index in runs merged like a binary counter
  (`LargeIndex`), so a seal indexes its own large brushes; an edit
  rebuilds the recent brushes' index, diffs the lists past the large
  layer and writes the brush buffer's tail in place. Edits the change log
  cannot bridge (another history, or past `CHANGES_KEPT`) regenerate the
  volume's every resident node in place (`resync_edits`,
  `EditSync::Resynced`): pools, residency and measured footprints stay,
  drawn chunks show until replaced. Only another recipe adds the volume
  again.
- The render thread finds the brushes over a footprint through a
  `BrushIndex` (a loose grid per face, brushes kept apart by the levels
  they apply at, so a coarse footprint skips those baked cells hold). A
  chunk job lists its brush refs and a table of the baked bricks under it;
  baked bricks stay on the GPU across batches (`BrickCache`, 64 MB by
  default), so a regenerated chunk sends its table, not its bricks.

**Kept chunks.** An edited chunk (generated under brushes or baked bricks)
the plan evicts is packed on the GPU before its pages go back
(`pack_main`: meta, mids, leaves, materials, normals and detail pages,
about 12 KB), read back with the frame's results (64 a frame) and kept on
the CPU (`Capacity::kept_bytes`, 256 MB by default, least recently used
dropped), with its footprint's measured ranges. A kept node the plan
wants again is restored by copy (`restore_main`): no footprint, terrain
program or brushes; the planner is given the footprint's measurement
again (one whose nodes are all restored is never measured, and the
estimate from its parent classifies it otherwise). An edit over a kept
node drops it.

Saves keep them too: `ChunkWorld::keep_resident` packs the resident
edited chunks, `kept_chunks` takes them, and `KeptChunks::to_bytes` writes
a header (a fingerprint of the generation shaders, terrain constants and
world, and the history's length and hash) and the chunks deflated.
`ChunkWorld::load_kept` keeps a volume's before its first update or after
a resync, for exactly that history; others are refused. Hosts reach this through
`PlanetFrame::kept` and its export (`KeptExport`), and save them beside the
edit snapshot (Pulsar's `VoxelKeptChunks`).

## Materials

`Planet::material` is the canonical material of a base cell (picks, a
dig's yield, gameplay). Generation evaluates `ground_material` with the
same inputs (its climate from the planet maps, [`planet-maps.md`](planet-maps.md)):

- **Point.** Each evaluation (a column's top cell, its soil and rock
  samples) uses its cell's volume point.
- **Slope.** `Planet::material_slope`: level-4 heights two cells each way,
  the gradient's length (an exact integer root on both sides), block
  slopes interpolated in 32nds, one scale at whatever level draws it.
  For levels 0 to 3 a column reads the base column at its centre; coarser
  cells span many base columns and keep the gradient over 64 base cells.
  (The larger of the two differences measured diagonal slopes short:
  slope thresholds traced squares on the face's grid, the Moon's rock
  specks ending at straight edges on crater walls.)
- **Surface word.** Its byte, as the CPU masks it.

Leaves draw their pair. Chunks of `DETAIL_LEVELS` (levels 0 and 1) take a
`cell_materials` slot (`DETAIL_JOBS` a batch; a job past them waits for the
next batch): `chunk_columns_main` writes each solid cell's edited material
and the column's natural top; `chunk_cells_main` gives each exposed cell
without one its natural material and clears the cells the leaf's pair
draws as they are, a lane a column (a lane a leaf column, as the build
has them, left most of its 256 lanes waiting on a few steep ones);
`chunk_build_main` gives every leaf page holding a leaf with cells left a
**detail page** (4 KiB: a byte a cell), off the stack of pages given
back, else past those ever taken up to the pool's capacity. Drawing reads
the cell's byte first (0: the pair); `slot_details` skips the lookup in
chunks without detail pages. Out of pages, a chunk keeps its pairs
(`UpdateStats::out_of_details`) and the world doubles the pool (8192 pages
at first, `Capacity::detail_pages` at most: 262144, 1 GiB, by default),
copying its pages and stack, then builds the starved chunks again. What a
view needs grows with the level 0-1 surface it shows: at 1600x900 a
meadow takes ~1.3k pages, the Moon's speckled regolith ~18k, inside an
Earth wood ~29k, an Earth mountain's snow and rock ~81k (its pages ~80%
full of leaves that need them).
Coarser chunks keep their pairs, and with nothing to correct them a pair
is what a leaf draws: its surface is one of its columns' tops, the one of
least hash (`vote_top`), not their majority. Over many leaves each
material then covers the share of the columns it tops, which is what the
finer levels' cells average to: a majority dropped every minority
material (the Moon's ejecta and rock specks, an edit's few blocks) at
every coarse level, so the ground changed colour where a detail level
met a coarser one. At detail levels the pair only decides which
cells need a byte of their own, so it stays the majority (fewest bytes).
A leaf's subsurface byte (what its cells under others draw: risers above
a lower neighbour show their column's surface, deeper cells soil or rock)
is read off its most exposed column, the one with the lowest terrain top
around it (`exposed_column`): read off its first column, a steep slope's
leaves drew their risers in an uphill corner's soil or rock, so far
forests' canopies on mountainsides showed the terrain as vertical
streaks.

## The drawn cut

`drawn.rs` keeps the cut and the GPU chunk table up to date by the nodes
that change; only a frame's changed entries are uploaded, and
`chunk_table.wgsl` scatters them. The table holds every drawn node and
every ancestor of one, `(face | level << 8, cx, cy, cz) -> (slot, state,
kind, content)`:

- `state`: shown, an ancestor (look finer), or fading with its start frame
  (`drawn::choice`).
- `kind` (`KIND_*`): mixed, solid, buried, or an ancestor not drawn itself.
- `content`: the box of its cells anything non-empty may occupy (a Mixed
  chunk's occupied layers over its footprint, a Solid or Buried node's
  whole box, an ancestor's the union of its children's). Empty nodes have
  no entry: a refined ancestor whose child has none is air there.

**Fades.** A switch between levels draws both sides for `FADE_FRAMES` (16)
frames, and each pixel shows one by a dither in [0, 1024) that moves each
frame, so TSR blends them. The fade's progress comes from the frame, so a
fade costs nothing while it runs. Incoming nodes show where the dither is
under the progress, outgoing ones where it is not: every point is shown
once. A plan that changes a switch under way ends it at once.

## Drawing

`ChunkWorld::draw`, after `update`:

0. **Cull** (`cull_bin`, `cull_offsets`, `cull_scatter`): the drawn chunks
   in view, against the frustum by their bounding spheres, binned by
   distance with those holding the eye first, into two indirect draws.
1. **Seed** (raster): every visible chunk draws its box (under its tilted
   top); a fragment writes only the box's index. Boxes do not overlap, so
   with the depth test on box faces each pixel keeps the nearest box its
   ray enters and hidden boxes never run. Boxes holding the eye draw their
   back faces, at the nearest depth. The cull takes every box within twice
   the near plane's distance of the eye (the near plane may cut its front
   faces): with the near plane a quarter of the height over the ground,
   many boxes beside the eye, all tied at that depth, any of them winning a
   pixel over the boxes in front of it. A pixel such a box seeds
   (`INSIDE_SEED`) starts its walk in the node of the cut where its ray
   starts (`point_start`, as a mesh point's walk), never at the seed: from
   the seed the ray went on from the box behind the ground in front, whose
   cells drew as chunk-sized walls and slabs (the Moon pole's box faces).
   A box past the pixel's mesh (the scene's depth before the chunks
   publish, read by the fragment, not depth-tested: see Publish) is marked
   hidden rather than discarded: the pixel traces nothing, and keeps the
   box as its frame chunk for the mesh point's walks (3, 4).
2. **Trace** (compute): each pixel's ray walks the cut from its seed box
   (`walk`). Inside a chunk, an integer DDA through the 64-tree: an empty
   child crossed in one step, a full one a hit where the ray enters. Past a
   chunk, `place_at` finds the drawn node over the point (coarser while
   none, finer through refined ancestors and switches this pixel does not
   show): a shown chunk; Solid or Buried, a hit where the ray enters
   (Buried reports itself); Empty, or outside an ancestor's content box,
   air grown to the largest slab of the coarsest ancestor whose content the
   point is outside of, so open air and dug halls cross in a few steps.
   Under the ground (a drawn chunk over the eye's column reaches above
   it: a cave, a dig, an overhang) the first round walks from the eye
   itself (`DrawParams::cut.z`, `point_walk`): in a large chamber the
   chunks around the eye are air (freed) or hold it under their occupied
   layers, and the rock beside them (Buried nodes) has no box, so a ray
   seeded by the boxes started past them.
   Across a cube face's edge the walk continues on the face beyond
   (`across_edge`: equal-angle faces meet cell for cell on the edge). Each
   chunk's frame is its own f32 transform, so two chunks place their shared
   side apart by rounding, and the raster's sub-pixel snapping seeds a ray
   within about 1/256 of a pixel of a shared edge in either box: a ray
   nearly parallel to the side can be seeded or stepped into a box it never
   enters in that box's frame. A box the ray does not cross (it enters no
   slab before it leaves another) passes it on to the box beside, at the
   cell it runs past in once it has entered every slab it crosses, where
   the box's frame is exact (`passes_beside`); it was walked as crossing it
   to its far sides, past the ground in the box beside (rays along the
   plane through the Moon pole's eye ended on the Solid node under it). A
   box a step crosses into takes the ray from where the box before let it
   go (`from_side`): its near side on that axis gives way to the step's
   time, and its trace starts in the border cells. Without it a ray nearly
   parallel to the side was in neither box between the two frames' sides,
   and went on under the ground to the next cell's side. Raster boxes reach
   a 64th of a cell past their cells (`SEED_REACH`; the walk is not
   widened): boxes share no vertices and a side between two tops of
   different heights is edge-on to an eye in its plane, so a ray along it
   fell between them, seeded by a box behind (the Moon's far side). The
   walk looks the next box up where the ray is, never back along it. A ray
   along an edge between frames meets either cell, to about that 64th. Over
   the ceiling (`Planet::outer_radius` and 16 m) and rising, a ray meets
   nothing more and ends: the sky. A ray ends a thousandth past its
   pixel's mesh (`MESH_MARGIN`; the shading tells which of a mesh and a hit
   that close is in front): terrain behind meshes is neither drawn nor
   reported, and picks there find none.
3. **Sun shadows**: walks toward the sun from a quarter of the hits (one a
   2x2 block, the pixel moving each frame), a hundredth of a cell off their
   face, each towards a point of the sun's disk (the atmosphere's angular
   radius, else Earth's 0.27 degrees; a pixel's own R2 sequence over the
   frames), so a penumbra widens with the occluder's distance. Every hit takes the sample of its block or a neighbouring one that
   walked within four of its cells; the rest (silhouettes) are listed and
   walk in an indirect dispatch of their own. Mesh pixels (with no hit
   behind them, the rays ending at the mesh) walk from the mesh point
   (`mesh_walk`), placed in the chunk the ray ended in, so terrain shadows
   fall on meshes. The denoiser (7) turns the samples into
   Helio's `directional_visibility`; its deferred lighting applies it.
   `set_steady_shadows` walks every hit instead, 32 rays over the disk
   (the denoiser passes the walks' own on: the reference for its error).
   Meshes in front of the terrain walk towards the disk's centre.
4. **Local lights' shadows**: the terrain shadows Helio's point and spot
   lights the same way (meshes cast into the lights' shadow maps). With a
   frame's lights (`SceneLights`: the scene's lights, light culling's tile
   lists), a hit of each 2x2 block walks to every shadowed light of its
   tile's list that reaches it, the walk ending at the light
   (`local_sample`); each hit takes its block's or a neighbour's word
   within reach, else walks its own; mesh pixels in front of the terrain
   walk from the mesh point. Bit i of a pixel's word (`local_seen`) is
   the tile's light i (the first 32); the denoiser (7) turns them into
   Helio's `local_visibility`, by which deferred lighting scales each
   light.
5. **Sky visibility**: four cosine-weighted rays about the planet's up
   from one hit a 4x4 block, 48 cells long, turned each frame; each hit
   takes its block's or a neighbour's sample.
6. **Ray reports** (`feedback`), then **publish** (below).
7. **Denoising** (`temporal`, `filter_first`, `filter_second`,
   `resolve`; `local_gate`, `local_temporal`, `local_filter`; cf.
   NVIDIA's SIGMA). Each term (sun, sky, each local light) accumulates
   along the hits' motion: the last frame's history at the hit's point
   reprojected (`prev_view_proj`, its jitter replaced by this frame's).
   A still camera (the projections differ by their jitter alone, to their
   rounding: `DrawParams::rays`) reads the texel it lands on; a moving one
   reads sharp (Catmull-Rom less its corners) over the texels of the same
   surface (view depth within its slope, normal within 37 degrees),
   bilinear where some are not, none past a disocclusion. A pixel's own
   sample (its walk, once a sample block's cycle) weighs 1, a neighbour's
   lent one 0.05, so the history keeps to the pixel's own shadow. A long
   history (32 frames for the sun, 64 for the sky, 16 for a light) and a
   short one (a cycle: 4, 8, 4) are kept; the sun's and the lights' are
   clamped to the range of this frame's samples in the 9x9 pixels around,
   so a moved sun or light, or an edit, shows at once where the samples
   agree; where the short history leaves the long one (beside a moved
   shadow's edge), the long one restarts from it. Two 3x3 à-trous passes
   (steps 1 and 2; one for lights) then filter the terms of pixels not yet
   settled, weighing neighbours by their distance off the pixel's plane,
   their normal and how far their value lies in the term's standard
   deviations (its mean's variance: the binary sun's m(1 - m), the sky's
   moments, over the frames accumulated; a fresh history's widened);
   settled ones pass as they are. `resolve` reads the result where this
   frame's jitter put each pixel's sample (TSR takes a pixel's value as of
   that point; read at its centre, TSR would blur it by the jitter's
   spread). A pixel tracks the four lights of least scene index that reach
   it, each an entry of its history; `local_visibility` is three words a
   pixel: those lights' tile slots, their visibility (a byte each), the
   other slots' bits. Without a point or spot light in the scene the
   lights' passes do not run (`local_gate`'s indirect dispatch). Meshes in
   front of the terrain and the sky pass their own walks on. The
   denoiser's data are storage buffers (wgpu bounds-checks an image load
   with two size queries) and its tiled kernels skip workgroup memory's
   zeroing. Cost at 1920x1080 on an RTX 3060: 1.15 ms (the sampled walks:
   3.4 ms; every hit walking: about three times those).

Steps near surfaces dominate: a step heatmap (debug mode 32; with
trace counters, `Capacity::counters`, also counted) is hot on
silhouettes and ridges, where rays skim pixel-sized cells.

### Ray reports

A pixel of each 2x2 a frame (all four over `REPORT_SWEEP`, 4 frames)
names the node its hit's level wants there: cells of a pixel, two levels
of margin, the hit's own node at least. A workgroup appends the distinct
nodes of its 32x32 pixels to one list, up to a node per 8x8 pixels in all
(the old readback's size). Rays that met Buried nodes report them too.
Rays end at meshes, so terrain behind them reports nothing: it is neither
seen nor reached: residency refines it only to `unseen_pixels`, and
Buried nodes there are not generated.
Reports come back without waiting; the planner marks the nodes and their
ancestors seen and generates the Buried ones.

A view's whole set is thus known a few frames after it shows. One pixel
of each 8x8 block a frame (a 64-frame sweep, before 2026-10-22) left bands
that a turn or a finer node's fade uncovered unmet for up to 64 frames, a
round each, and settling waited a whole sweep after the last change:
voxel_craft's second pose asked for chunks 30 to 100 frames after the
turn. The reports cost 0.03 ms at 1080p (every pixel each frame cost
0.17 ms: two bounds-checked texture loads and the node's arithmetic a
pixel). A fixed number of nodes per tile dropped the same ones every frame
where tiles held more (toward the horizon): the list has no per-tile cap.

### Publish

The publish pass writes Helio's gbuffer and depth: material colour
(`material_albedo`: per-voxel pigment and the materials' patches, clumps
and blades from the appearance table, sampled at the cell centre's domain
point; a voxel's pigment is as much its 0.4 m clump's as its own, since a
hash per voxel read as a checkerboard wherever the light shapes nothing,
in shade and under cloud; where cells are a pixel or less, each face's material taken at a
point jittered up to a leaf across it each frame, so TSR averages the
leaves' materials into their mixture), corner occlusion from the cells
around the air in front of the face (across chunks by the chunk table)
times sky visibility, crisp edges
while a cell spans several pixels, and the shading normal. A natural step
(air above) is lit partly with the slope's normal (`detail.w`, step
softness: 0.75 by default) and its sun and sky rays pass its own steps.
Lit by their own faces (step softness 0, the default from 2026-10-12 to
2026-10-26), the 10 cm steps of a gentle slope drew each riser and the
shadow it casts as a dark line: contour lines over every meadow, under
the grass and past it, near and from the air, and rings in lunar crater
bowls. Natural means the leaf stores the heightfield's slope (its columns
hold natural tops): features and edits keep their own faces (softened, a
rock field lost its form and its shadows). Local lights' rays do not pass
the steps (only cells of a pixel or less, and no farther than the hit's
own pixel): a lamp near the ground meets the real steps, and a grazing
step's skip (up to 12 cells) passed what stood beside the lamp, or the
lamp itself; on rough ground (eroded slopes' single-cell steps) a step a
cell or two along casts a shadow several pixels long.
Helio's deferred lighting, atmosphere and TSR do the rest.

It writes only the hits no mesh covers, as the shading pass found them
against the meshes' depth (a covered hit's normal is zero), and writes the
depth without testing it. One decision, not two: on NVIDIA's Vulkan driver
the publish's own depth test once read a depth surface that a copy read
correctly the moment before, let every hit through and left the depth
garbage for whole runs (black grass on Earth, 2026-10-22).

With a host's scene materials (`scene.rs`, `shaders/scene_materials.wgsl`,
devices with texture binding arrays), each voxel material's top, side and
bottom faces use Helio material rows and textures instead of the flat
appearance colours.

## Costs

1080p, RTX 3060, `mountain_slope` hold (2026-10-19): primary 3.06 ms, sun
shadows and sky 1.72 ms. Generation in the cruise stress flight: about
15.5 us a chunk (footprint columns most of it, chunk builds 3.5 us, chunk
columns 1.9 us). Settled test views take 3,000 to 10,000 detail pages.

Terrain behind meshes (2026-10-27, 1080p, RTX 3060): voxel_flight's
`ground` hold in grass, chunk draw 2.44 -> 1.76 ms and shadows 3.66 ->
2.89 ms (VoxelChunks 8.7 -> 7.2 ms); the same view bare, unchanged (2.44
-> 2.46 ms: the seed's depth load and the walk's runtime end). voxel_craft's
poses with grass, VoxelChunks 0.15 to 0.7 ms less; bare, within noise. A
wall covering the nearer half of a ground view
(`terrain_behind_meshes_is_not_traced`): half the walk steps, reports
5,492 -> 96.

Gates (`tests/chunks.rs`): drawn hits match CPU ray casts (ground, face
edges, cave networks 600 m deep, lava tubes, cliff shapes, planes, brushes, a 600 m shaft, a 3 km
crater, a hollowed core, an underground hall, 18 000 block edits, sealed
edits, features); `uniform_leaves_draw_canonical_materials`,
`dug_walls_draw_canonical_materials` and
`edited_cells_draw_their_own_materials` draw `Planet::material` exactly;
`edited_chunks_come_back_by_copy`,
`edited_chunks_released_by_age_come_back_by_copy` and
`a_loaded_world_restores_its_edited_chunks` restore edited chunks with no
generation; `terrain_shadows_fall_on_meshes_in_front_of_it`;
`terrain_behind_meshes_is_not_traced` (hits in front of a mesh plane as
open, none past it, half the steps and reports). Reports,
ignored by default: `natural_cells_against_the_leaf_rule` (0 of 906k
pixels wrong), `edit_cost_against_history`, `feature_generation_cost`.

## Known driver issue

NVIDIA's Vulkan driver (616.64, RTX 3060) intermittently loses G-buffer
writes: after a frame, scattered 8x8 and 4x8 pixel blocks of every
G-buffer target and of depth still hold the G-buffer pass's clear, so
lighting takes them for sky (about one fixed-step descent in two, captured
every 0.25 s). The publish runs and finds a hit for every pixel. D3D12
never shows it; synchronization validation is clean; barriers, fences, a
single command buffer and the recording cache change nothing. It needs the
frame recorded while the GPU still runs the frame before, with generation
in it; waiting for the GPU before the voxel pass records removes it (at
+44 % frame time), as does not clearing the G-buffer when the publish
writes every pixel. Reported as NVIDIA bug 6939141; a forum report with
the same profile: https://forums.developer.nvidia.com/t/382912.

## Open work

- Occlusion-limited refinement where much is hidden (valley floors,
  caves, cities): on the editor's open views, refining nothing until rays
  meet it changed nothing (`unseen_pixels` 2 covers them).
- A footprint's first computation: progressive (parent-seeded) synthesis.
- Seed boxes wholly behind meshes still rasterize (each fragment one depth
  load): a HiZ cull of the boxes against the meshes' depth would save that
  once box counts make the seed cost (0.04 to 0.09 ms at 1080p) matter.

## References

Laine and Karras, *Efficient Sparse Voxel Octrees* (2010); Crassin et al.,
*GigaVoxels* (2009); Careil, Billeter and Eisemann, *Interactive Editing of
Large Sparse Voxel DAGs* (HashDAG, 2020); Fang et al., *Aokana: a GPU-driven
voxel rendering framework for open world games* (2025); Teardown's renderer
(Gustafsson, 2020-22); sparse 64-tree tracing (dubiousconst282, 2024);
Karis, *Nanite* (2021) for the screen-space cut; *Filtered Appearance for
Voxels* (HPG 2023); Distant Horizons for LOD crossfades.
