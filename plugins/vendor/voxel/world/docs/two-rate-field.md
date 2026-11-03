# Two-rate field: large octaves at nodes

Generation throughput for fast descents: no visible refinement needs
footprints generated within a few frames of being wanted
([`chunk-octree.md`](chunk-octree.md)).

## Why

A tile's columns cost 27.8 µs per tile of the ~40 µs a tile takes to
generate (GENBENCH, `HELIO_VOXEL_GEN_BENCH`). Gradients took 48% of that,
erosion 29%, ridges 23%. Most of it goes to octaves whose wavelength is
hundreds of columns: continents, mountain-range regions, the large
ridges and warps. Every column evaluated them, though they change by a
fraction of a cell between neighbours. 
## Definition

At a column of level `L` (reference level `L + level_offset`), an octave
is a **macro octave** when its shift is at least `L + MACRO_GAP` (15:
wavelengths of 256 node spacings or more) and it is not erosion or a
crater. Gully floors and crater rims are kinks that interpolation would
blunt.

**Nodes** are the columns of the level whose indices on their face are
multiples of `NODE_COLUMNS` (16). The face grid is linear in its indices,
so a column's weights between its four nodes are exact integers:
`(16 - di)(16 - dj)`, `di (16 - dj)`, `(16 - di) dj`, `di dj` (sum 256).

A node carries (`landform::Node`, 116 words):

- the macro octaves' smooth sums per layer: value, region and their
  gradients;
- each macro ridge octave's noise and gradient, the first `MACRO_RIDGES`
  (10) in table order. The column applies the ridge step itself, so crests
  stay sharp: interpolating `1 - |n|` would round them;
- the macro warp octaves' offset and Jacobian rows.

A node is evaluated at its domain point under the **whole** warp, so the
macro octaves sit where the single-rate field puts them.

A column blends its four nodes' words, `floor(sum w_c v_c / 256)`
(`landform::lerp4`). The high byte and the low byte of each word are
summed apart, so the sum is exact in wrapping 32-bit arithmetic and can be
accumulated one node at a time. Then the column evaluates every finer
octave itself, starting from the blended sums, and applies the blended
macro ridges at their places in the table.

This **is** the terrain: CPU (`landform::column_parts`) and GPU
(`landform.wgsl`) compute it bit for bit. The single-rate field
(`landform::height_parts`) remains as a reference.

## Accuracy

Test `two_rate_field_stays_within_a_fraction_of_a_cell` compares
two-rate heights against single-rate heights. The worst difference is
0.13 cells at level 0, 0.065 at level 1, and ≤0.025 from level 2 up.
Because every level follows the same definition, a level switch shows
none of it.

## Interface

`TerrainField::height` and `surface` take a `Column {face, i, j, level}`
instead of a domain point, since a field may share work between the
columns of a level.

A WGSL program opts in with `TERRAIN_NODE_WORDS`, `terrain_has_nodes`,
`terrain_node`, `terrain_node_word` and `terrain_column_nodes`
(`terrain.rs` docs). It reads a column's blended words through the
engine's `generation_node_word(slot)`.

## Column programs

The octave table is sorted coarse to fine, so at each reference level the
octaves a column evaluates are a fixed set: the resolved ones finer than
the macro gap, the macro ridges (from the nodes), erosion and craters, and
the display layer's ridges. `LandformPlan` compiles that set once per
stack, for 32 levels, as ops in table order. An op is its octave index
plus flags:

- `OP_MACRO`, with the ridge's node slot;
- `OP_DISPLAY` and `OP_DISPLAY_ONLY`;
- `OP_REGION_DONE`: the layer's mask is complete;
- `OP_STEER`: the op changes continents or mountains.

Each level also has a mask of the node words it reads. The CPU
(`landform::parts`) and the GPU (`landform_column`) run the same ops from
the same constants (`TerrainConstants::plan_*`).

A column runs only its level's ops, not the whole table (about 50
octaves, each with its resolved, macro and region checks). Steering is
recomputed only after `OP_STEER` ops: it is a pure function of the
continents' and mountains' accumulators.

## GPU

Generation runs these passes, each timed (`FrameTimings::tile_stages`,
GENBENCH):

1. **`tile_nodes_main`** runs one workgroup per tile. It evaluates the
   tile's 7×7 nodes, the 5×5 over its 64×64 columns plus a ring one spacing
   out, into `tile_nodes` (49 × 116 words per tile of a batch).
2. **`tile_columns_main`** blends each column's four nodes from that
   buffer, only the words its level uses. Every lane of an 8×8 workgroup
   lies in one node cell, so each word is a broadcast read.
3. **`tile_caves_main`** evaluates, once per footprint, the cave
   networks' reference ground at their sites (a level about an eighth of a
   region, [`volumes.md`](volumes.md)): columns of another level evaluate
   their own four nodes into function arrays and leave the blend in a
   scratch node. `chunk_columns_main` does not run the field.

WGSL clears every private variable an entry point can reach at each
invocation, so node arrays live in function scope and in the scratch nodes:
module-scope arrays reachable from the hot kernels cost them 2–3.5×.

`field_height` and `generation_column` (verification, timing) evaluate
their four nodes per column.

## Fixed-point products

`mul_shr_signed` (and `mul_fine`) rounds to nearest, halves up, as a
wrapping signed 64-bit multiply, add and shift. Rounding toward zero
needed sign handling that took a fifth of a column's samples in the
profile. Rounding down passed the bit-exact tests but biased sums of
products, and failed the ridge envelope's statistical and sign-symmetry
tests.

## Measurements

GENBENCH, RTX 3060, `mountain_slope` and `ground` holds, same level
layers, µs per tile (chunk stages per tile at ~1.4 chunks a tile):

| | tile work | chunk columns | all |
|---|---|---|---|
| single-rate field | 28 | 6.6 / chunk | 41–43 |
| two-rate, face lattice, halo pass | 28 (nodes 1.3, columns 23.6, halos 3.3) | 5.0 / chunk | 38–40 |
| + compiled column programs | 23 (nodes 1.4, columns 19, halos 2.8) | 4.8 / chunk | 32–34 |
