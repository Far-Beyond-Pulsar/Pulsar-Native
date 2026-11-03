# Features: procedural content as part of the terrain

Trees, rocks, ruins and buildings standing on a planet, generated like the
terrain: a function of the world's definition evaluated wherever and at
whatever level a chunk is generated, on the GPU for drawing and identically
on the CPU for queries, picks and physics. Nothing is loaded around a
viewer, nothing is kept in memory past the chunks drawn and their
footprints, and a forest shows to the horizon at the terrain's cost.

## Model

A planet's recipe holds a **feature set** (`FeatureSet`), part of the world's
definition (its fingerprint; saves keep the recipe):

- **Stamps**: voxel models built from shapes (`stamp::Stamp`; `flora`
  grows natural ones in metres for any voxel size and materials: trees of
  six species, bushes, dry-land shrubs, cacti, boulders, impact craters): boxes and
  tapered capsules (a ball is a capsule of no length) of material ids 1 to
  127 or of air, around an anchor, the later over the earlier. A shape may
  be sparse (a share of its cells left out by hash: foliage) and rough (its
  edge broken by hash). A shape is a handful of numbers at any size, so a
  10 cm tree of a million cells is a few dozen shapes; they are evaluated
  in integer sixteenths of a cell, bit for bit on the CPU and the GPU.
- **Spawns** (rules, data): which stamps grow where, from the widest
  spacing to the narrowest (structures, then trees, then rocks):
  - a placement grid: one candidate per `spacing` x `spacing` base columns,
    jittered by a hash of the world seed, the spawn and the square within
    the square's middle half (neighbours' anchors keep half a spacing
    apart);
  - a density (a share of candidates by hash) and a noise field of the
    world (`field_scale_m`, `field_threshold`);
  - ground rules: the materials it grows on, the heights (which may
    wander by noise of the world, `height_wander_m`: a tree line ragged
    like the terrain's snowline, not a contour), the climate (temperature
    and moisture ranges, the terrain's `climate` at its ground at the
    wandered height: [`planet-maps.md`](planet-maps.md)), the most relief
    between its anchor's ground and its footprint's corners, how deep it
    sinks;
  - its stamps (variants chosen by hash), turned and mirrored by hash;
  - its **far form** (below): material and canopy height.

**Recipes.** A feature set is usually grown from a **feature recipe**
(`feature_recipe::FeatureRecipe`, a few kilobytes of JSON a level saves):
spawns whose rules are in metres and whose stamps are growths of `flora`
(a form: tree of a species, bush, shrub, cactus, boulder, crater; its
materials and sizes; a seed salt; how many variants). `grow(voxel, seed)`
makes the set for a world's voxel size and seed, the same set wherever it
is grown. `FeatureRecipe::earth()`, `moon()` and `desert()` are the
setups voxel_flight flies through (woods to a wandering tree line, a
cratered moon, a stony desert); a game builds its own, or a set of its
own stamps (voxel_craft's structures).

A candidate's ground is the terrain's height over its column (the
generator's height function) and its ground material the terrain's rules
at its top cell on level ground (slope 0: a column's own inputs, the same
at every level). Features read the terrain alone, never edits, so every
chunk and every query sees the same placements; edits apply over them.

**Overlap and exclusion.** A spawn's own placements overlap where their
stamps reach past their squares: a forest's crowns interlock. A candidate
whose footprint (its stamp's reach each way) meets an earlier spawn's
placement does not grow: a tree never grows through a house. A base cell
is what the last placement holding it makes of it (spawns in order, a
spawn's squares by row then column).

## Levels

- **Near levels** (level 0, and cells at most half the spacing and an
  eighth of the stamps' footprint: legible shapes): the
  stamps. A base cell takes the last shape holding its centre; a coarse
  cell the last holding its box's centre, smooth (no broken edge) and solid
  where a sparse shape keeps half its cells, else the terrain's. The
  spawns drawn at a level are a prefix of the set (spawns go widest
  first).
- **Far levels**: a spawn with a far form draws as a canopy over the area
  it grows in, as a mountain is drawn by its envelope: a level column whose
  ground its rules accept (its field at the column's middle, or past the
  field's scale the field's majority; heights; ground material) holds the
  canopy's layers from its ground up, a cell solid where they fill half of
  it. Wandering heights are read at the column's middle base column. Cells taller than twice the canopy keep its colour: a covered
  column's natural top is drawn in the far material, so a forest shows to
  the horizon. Near stamps stand over far forms where both apply.

## Pipeline

GPU (`chunk_generate.wgsl`):

1. `tile_columns_main`: the terrain's columns.
2. **Candidate blocks** (`chunks::candidates`): a spawn's raw candidate on
   a square is a function of the square alone, so candidates are evaluated
   by blocks of 8x8 squares, each once while a pool keeps it (16384 blocks,
   least recently used reused): the frame's footprints to compute reserve
   the blocks their squares span, and those the pool lacks (64 a frame at
   most) are evaluated in three wide dispatches:
   `feature_block_quads_main` (a workgroup a block: the level-0 node quads
   its anchors' and footprint corners' grounds read, listed once each),
   `feature_block_nodes_main` (a lane a node) and
   `feature_block_candidates_main` (a lane a square: heights, ground
   material, relief).
3. **`tile_features_main`** (a lane a square of each computed footprint):
   the near spawns' raw candidates over the footprint and the margin later
   spawns' exclusion reads (`CompiledFeatures::layout`) copied from the
   blocks into the footprint's pool slot (`tile_placements`, kept while
   the footprint is), excluded where they reach the footprint and an
   earlier spawn's raw candidate meets them; its measured ranges raised
   over the placements reaching its columns and its far forms' canopies
   (classification and residency see trees as terrain).
4. `chunk_columns_main`: after the terrain's mask and before the edits,
   `feature_column`: the far canopy, then each placement within reach in
   order, its shapes in order over the cells their boxes span; their
   materials into
   the chunk's cells, and the column marked so `chunk_build_main` stores
   them as it does edited cells.

CPU (`Planet`): `sample_kind` / `material` / walks evaluate the same
placements and stamps over a cell (`FeatureEval`, each square's placement
cached), then the far forms (`CompiledFeatures::far_cover`).

Tests pin CPU and GPU: every footprint's placements are the CPU's square
by square, features render exactly against CPU ray casts at level 0, draw
their stamps' materials, and every coarse hit (near stamps and far
canopies) is solid on the CPU.

## Costs

A footprint keeps `CompiledFeatures::stride` squares (8 bytes each). A
candidate costs one height evaluation, plus four with a relief rule, once
while its block stays in the pool (512 bytes a block), however many
footprints and levels keep its square (a level-0 footprint's tree squares
reach some 150 columns past its 64 each way: each was evaluated by some 30
footprints when footprints evaluated their own); a footprint copies its
squares and checks exclusion. A chunk column scans the squares within its
spawns' reach, and each placement's shapes over the cells their boxes span.
