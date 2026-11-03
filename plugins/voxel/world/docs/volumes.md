# Volumes: caves, overhangs and cliff shapes

The terrain's volumetric shapes, the ones a heightfield cannot make: cave
systems with chambers, tunnels and entrances; ledges, undercuts and
overhangs on steep ground. They are part of the terrain contract like the
heightfield: a function of the world's definition, evaluated wherever and
at whatever level a chunk is generated, on the GPU for drawing and bit for
bit on the CPU for queries, picks, physics and edits
([`chunk-octree.md`](chunk-octree.md), [`two-rate-field.md`](two-rate-field.md),
[`features.md`](features.md)).

Code: `src/volumes.rs` (settings, compiled constants, the CPU evaluation),
`shaders/volumes.wgsl` (its mirror), `tile_caves_main` and
`tile_volumes_main` in `shaders/chunk_generate.wgsl`.

## What it replaces

The caves before this (2026-10-08) were built for the span-column
renderer: tunnels where two 3D noises were both near zero and caverns
where a third passed a threshold, inside 2D noise "cave regions", within a
fixed 120 m of the local surface, opening at noise "entrance zones".
Overhangs were a horizontal lean of the heightfield (at most 6 m), which
needed the field's height re-evaluated on a lattice around every leaning
column (`tile_halo_main`). Their limits:

- No structure: tunnels went nowhere in particular, ended in noise
  pinches, met by chance; no chambers, no flat floors, nothing a level
  designer or a gameplay system could reason about (where is the cave,
  where does it lead, is it connected).
- Depth relative to the local surface: tunnels followed every hill, and a
  cave's shape sheared with the slope above it.
- Entrances were wherever a noise zone crossed a tunnel.
- The lean could only push the ground sideways: no ledges, undercuts or
  caprock, and it cost a lattice of height evaluations per tile.
- Bounds were the whole 120 m band under every cave region's column.

## How others do it

- **Minecraft 1.18+** (Caves & Cliffs, Kniberg 2022): density functions
  composed by a noise router; noise caves are *cheese* (a large 3D noise
  above a threshold: caverns with pillars), *spaghetti* (two noises both
  near zero: long winding tunnels, the old system's tunnels) and *noodles*
  (thin spaghetti), plus aquifers (local fluid levels from noise) and the
  old carvers kept for ravines. Absolute Y bands, no explicit structure;
  connectivity is statistical. Point-evaluable and cheap, which is why it
  works at Minecraft's scale; the look is noise.
- **Vintage Story, pre-1.18 Minecraft, 7 Days to Die**: carvers. A worm
  (a random walk of spheres) starts in a chunk and carves into every chunk
  within its reach; each chunk replays the worms of its neighbourhood from
  their seeds. Structured tunnels, but a chunk's content depends on a
  sequential simulation, so it is not point-evaluable or band-limitable.
- **Deep Rock Galactic**: explicit graphs. Rooms (shapes from templates
  and noise) placed first, connected by tunnel splines, then carved with
  noise. The look AAA cave gameplay wants: places, passages, junctions.
- **No Man's Sky** (McKendrick, GDC 2017): one 3D density per planet,
  turbulence for overhangs and arches, cave layers by altitude bands,
  polygonized by dual contouring. Floating rock accepted.
- **Astroneer**: small planets with shells by radius (surface, caves,
  core). Absolute radius only works because planets are kilometres wide.
- **Space Engineers, Dual Universe**: heightmap cube maps and material
  maps; no generated caves on planets, everything else by mining.
- **Teardown, Enshrouded**: authored voxel worlds (MagicaVoxel models,
  editor tools); caves are content, not a generator.
- **Papers**: Gamito and Musgrave, *Procedural landscapes with
  overhangs* (2001): domain-warped heightfields (our old lean). Peytavie et
  al., *Arches: a framework for modeling complex terrains* (EG 2009):
  material stacks with overhangs, arches and caves. Paris et al., *Terrain
  amplification with implicit 3D features* (TOG 2019): skeleton-based
  implicit primitives (arches, overhangs, caves) blended into a
  heightfield. Paris et al., *Synthesizing geologically coherent cave
  networks* (CGF 2021): karst networks as graphs from geology (inception
  horizons, fractures, the water table), then implicit volumes around the
  graph's skeleton. Santamaría-Ibirika et al., *Procedural approach to
  volumetric terrain generation* (2014). Recent work learns karst graph
  statistics (*A deep generative model for the simulation of discrete
  karst networks*, arXiv 2506.09832, 2025).

The best of them: **explicit graphs of chambers and passages carved as
implicit primitives** (DRG, Paris 2021), made **point-evaluable** the way
our features are (a placement per region from a hash, its ground read
from the field), with **noise only for detail** (walls, ledges).

## Constraints

- Integer, bit-identical CPU/GPU (`chunks` tests compare drawn hits with
  CPU ray casts).
- LOD-consistent: a coarse level is a band-limited sample of the same
  function at its cell centres; detail smaller than a level's cell is
  omitted, never replaced by something else.
- Cheap per column; free where there is nothing (most of the planet).
- Tight per-column bounds for classification (`Mixed` / `Buried`).
- Planet scale: any radius, planes, 2^27-cell faces, 1 cm to 1 m voxels.
- Generic: Earth, Moon, desert, flat block worlds; settings are data.
- Reads planet context later (baked drainage and climate maps): karst
  where rock and water allow it, lava tubes on volcanic worlds.

## Why this design

- **Against noise caves alone** (Minecraft's cheese/spaghetti, our old
  system): thresholded noise is cheap and point-evaluable, but it has no
  structure. Nothing guarantees a tunnel leads anywhere, chambers have no
  floors, entrances are accidents, and its bounds are the whole noise band
  (every column of a cave region is volumetric down to the cave depth).
  Placed networks cost about the same per cell, give connected systems,
  flat walkable floors, deliberate entrances and a graph that gameplay,
  AI and level tools can query. Their bounds are the primitives' boxes,
  and a region without a network costs nothing. Noise stays as detail on
  their walls.
- **Against carvers** (worms replayed from neighbouring chunks): carvers
  give the same structure, but a chunk's content depends on a sequential
  simulation over its neighbourhood. They are not point-evaluable, not
  identical at every LOD, and not cheap to query on the CPU. A network
  here is a closed-form function of its region's hash and five or six
  height samples.
- **Against a full 3D density field** (No Man's Sky): every cell near the
  surface would evaluate 3D noise, and floating rock is common. Here only
  columns inside a primitive's box or a steep cliff band evaluate
  anything; everywhere else the terrain stays an exact heightfield.
- **Against the lean** (Gamito and Musgrave, our old overhangs): a domain
  warp needs the heightfield re-evaluated at displaced points (a lattice
  and halo pass) and can only push ground sideways. Displacing along the
  normal needs only the column's own height and slope, and its strata
  term makes ledges, undercuts and caprock.
- **Against authored caves** (Teardown, Enshrouded, DRG templates): those
  do not scale to a planet. Hand-made caves remain possible as features
  or edits over the generated ones.

## Scaling

- **Planet size.** Networks are per face region and per-cell work is
  local, so cost does not depend on the radius. Faces up to 2^27 cells
  (Earth at 1 cm voxels: 2^30 cells, kept relative) fit because
  generation keeps coordinates relative to the footprint and the CPU
  uses 64 bits. The reference ground is one coarse column per site.
- **Voxel size.** Settings are in metres; regions are a power of two
  cells (2^10 to 2^13), so a 1 m voxel world gets 1-8 km regions and a
  1 cm world 10-80 m ones. Spacing is held so that segment products fit
  32 bits.
- **Cave density.** A region has at most 13 primitives, and a footprint
  meets at most 39 at the levels that resolve them (8 owners at tunnel
  levels, chambers only at two regions per footprint), within
  `CAVE_SLOT` (48). Denser caves mean more regions with networks, not more
  primitives per column. Per-cell cost scales with the primitives over a
  column (typically 1-4), and noise is evaluated only in the walls' shell.
- **LOD.** A primitive drawn at a level is the same capsule sampled at
  that level's cell centres. It is resolved while its smaller radius
  is at least a cell, and omitted below that, as finer octaves are.
  Coarse levels whose cells outgrow every cave keep the Buried band
  (`max_depth_m`) so eyes and rays still reach them. Cliff shapes stop at
  `CLIFF_LEVELS` (0.8 m cells at 0.1 m voxels), where the heightfield's
  terraces take over.
- **Memory.** 1.5 KB per footprint pool slot (3 MB in all) and a small
  CPU cache of region networks.

## Generic for any world

The layer depends only on the field's height and surface word, so any
generator gets it. Spheres, finite and infinite planes all use the same
face-region code (planes have one face). Settings are data, so the
Earth's karst, the desert's sandstone ledges, the Moon's lava tubes and a
block world's shallow caves (voxel_craft) are presets of one design, and
`Volumes::none()` keeps a pure heightfield. Planet context comes in
through one hook, `TerrainField::cave_gate` (`terrain_cave_gate`): a
share at each network's hub, read from the generator's own inputs (rock
type, drainage, volcanism) with no change to the layer.

## Design

The volume layer is engine code over any generator's field (it reads the
field's height and surface word); its settings live with the terrain's
(`TerrainLayers::volumes`, `volumes::Volumes`), a generator supplies them
through `TerrainField::volumes`. Two parts:

### 1. Cave networks

**Regions.** Each face is divided into square regions of `S = 2^R` base
cells (`spacing_m`, 200 m by default, rounded to a power of two cells and
held to `2^8..2^13`). A region's network is a function of its face, its
indices and the world seed:

- **active** by hash against `share`, times the generator's gate at its
  hub (`terrain_cave_gate(surface, height)`, Q8; 256 everywhere today,
  the planet maps' karst/volcanic masks later);
- a **hub** node jittered in the region's middle half, at a depth in
  `depth_m` below the **reference ground**: the field's height at its
  column at the reference level (cells about `S / 8`), so caves sit under
  the ground's large shape without bobbing over every hill;
- up to two **chambers** (by `chambers`), in the middle half, their depth
  near the hub's, each joined to the hub by a tunnel; the hub is a chamber
  itself by `hub_chambers`, else a junction;
- **links**: a tunnel from the hub to the `+i` and to the `+j` region's
  hub when both are active, by `links` (connectivity across regions: the
  networks join into systems spanning kilometres);
- an **entrance** by `entrances`: a tunnel from the hub up to a point a
  quarter region away and 20 m over its reference ground, so it breaks
  through the real surface wherever that is (a cave mouth in a valley
  side, a sinkhole on a plateau).

Every edge is two capsule segments through a bend point (the midpoint
moved by hash horizontally and vertically), its radius from `tunnel_m`.

**Primitives.** A region's network is at most 13 primitives: capsules
`(a, b, radius)` with a vertical squash (chambers are flattened
ellipsoids: `a = b`) and a **floor**: cells below the axis by `floor` of
the vertical radius stay solid, so chambers and tunnels have flat, walkable
floors that follow the passage's slope. Coordinates are face cells
(horizontal base cells, vertical base layers) in quarters, a column's
cells sharing its horizontal position; in segment-local differences every
product fits 32 bits (segments are under `2^12` cells, `S <= 2^13`).

**Carving.** A cell is carved when a primitive resolved at its level
holds its centre:

- `e = c - a`, `d = b - a`, `t = clamp(e.d, 0, d.d)` in Q12 of the
  length, the offset `q = e - d t`; its vertical part scaled by the
  squash; inside when `|q|^2 < r^2`;
- not below the floor plane at the closest point;
- walls broken by 3D noise (`roughness`, a share of the radius), evaluated
  only in the shell where it can change the answer;
- under **cover**: the radius narrows to nothing over the last `cover_m`
  under the column's surface (caves close into domes instead of breaking
  the ground), except along entrances;
- never deeper than `max_depth_m` under the column's surface (the
  classification bound; a tunnel under a peak narrows away there).

A primitive is **resolved** at level `L` when its smaller radius is at
least a cell of `L`; tunnels only while a footprint is at most a region
across (`L <= R - 6`), chambers while it is at most four (`L <= R - 4`).
That bounds the primitives a footprint can meet (below), and below those
levels caves are smaller than two cells anyway.

**Owners.** A region's primitives lie within it and its `+i`/`+j`
neighbours (nodes in the middle half, bends under `S / 8`, radii under
`S / 8`), so a column needs the networks of three regions: its own,
`-i` and `-j`. Faces do not share networks: nodes keep a quarter region
from the face's edges, so no primitive crosses one.

### 2. Cliff shapes (ledges, undercuts, overhangs)

On steep ground the surface is displaced along its normal instead of
vertically. With the column's height `H`, its material slope `s`
(`Planet::material_slope`, eighths of a cell per cell, exact on both
sides) and `N = sqrt(64 + s^2) / 8` (held at 4), a cell at height `z` is
solid when

    (H - z) + A N g(x, y, z) > 0

`(H - z) / N` is the distance to the slope's plane, so the displacement is
`A g` along the normal: horizontal on cliffs, vertical (and faint) on
level ground. `A` (`depth_m`) fades in with slope (`slope`, rise over run)
and by a 2D region noise (`region_km`, `share`). `g` mixes:

- **strata**: a triangle wave of height with period `strata_m`, its phase
  wandering by a 2D noise: horizontal ledges and undercut bands along a
  cliff (caprock over softer layers);
- a **3D noise** of wavelength `4 A`: bulges, alcoves, ribs.

On a monotonic slope the solid set at each height is still a half-plane
moved by `A g`, so ledges stay attached to the cliff behind them; rare
floaters form at ridge crests. Shapes are resolved at levels whose cells
are under `A` and below `MATERIAL_SLOPE_LEVEL` (the slope's exact
levels: 0 to 3, 0.8 m cells at 0.1 m voxels); coarser levels draw the
heightfield. Large escarpments stay the heightfield's (`LayerKind::Cliffs`
terraces); this layer gives them their metre-scale volume.

**Arches, hoodoos, sea stacks** are placed shapes standing on the ground:
features (`docs/features.md`): stamps of capsules and boxes with air
shapes under them, placed per region like trees. The volume layer does
not make free-standing rock.

## Pipeline

GPU (`chunk_generate.wgsl`), per computed footprint:

1. `tile_columns_main`: heights and surface words (no volume terms).
2. `tile_slopes_main`: material slopes (levels 0-3), as before.
3. **`tile_caves_main`** (a workgroup a footprint, replacing the lean's
   `tile_halo_main`): the owner regions of the footprint (at most 8 at
   tunnel levels), each lane a node: its position by hash, its reference
   height and surface word (`generation_halo_column` at the reference
   level: about 50 coarse columns a footprint, under 1% of its columns);
   then each region's primitives, culled to the footprint's box, into its
   pool slot (`tile_caves`, `CAVE_SLOT` = 128 primitives of 8 words).
4. **`tile_volumes_main`**: each column's extent, below and above its top
   (the cliff band, the deepest primitive over it, held to `max_depth_m`),
   into `tile_columns.zw`, and the footprint's measured ranges.
5. `chunk_columns_main`: a workgroup (8x8 columns) culls the footprint's
   primitives to its columns and the chunk's layers into workgroup
   memory; each column evaluates the cliff band's cells and, for each
   primitive over it, only the cells of its height range.

CPU: `Planet` caches region networks (positions, reference heights,
primitives) and evaluates the same cells (`volumes::Column` once a column:
height, slope, cliff amplitude, the owners' primitives over it).

## Bounds and classification

- A column's extent is exact to its primitives' boxes and its cliff band:
  a footprint without a network and without steep ground in a cliff
  region is a pure heightfield and costs nothing extra.
- `TerrainField::volume_bounds` (depth `max_depth_m`, height the largest
  cliff rise) still sets the Buried band for coarse levels, where caves
  are not resolved but rays and eyes must reach them
  (`EditView::volume_depth`).

## Edits and destruction

Volumes are terrain: edits apply over them in order (large brushes, baked
cells, recent brushes), a dig into a cave opens it, a fill closes it.
Networks never read edits (as features do not), so every chunk and every
query sees the same caves. Kept chunks' fingerprints include the volume
constants.

## Materials

- **Walls**: the generator's ground material at the cell (depth from the
  generated top), so strata and rock types continue into caves.
- **Floors**: today the walls' material too. Sediment floors (gravel,
  clay, regolith on the Moon) need the per-cell material path to see the
  natural volume, not only the post-edit masks: see Open work.
- **Cliff faces**: the generator's rules by slope, as before.

## Settings (`volumes::Volumes`)

| | Earth | Desert | Moon | Flat |
|---|---|---|---|---|
| networks | karst: chambers, links, entrances | fewer, drier, more entrances | lava tubes: shallow, wide, long links, skylights | off |
| cliff shapes | 3 m strata, 4 m depth | sandstone: 2 m strata, 6 m depth | off | off |

Lava tubes are the same network with different numbers: no extra
chambers, tunnels 6-15 m wide with flat floors, 15-40 m deep, every hub
linked, entrances as skylights.

## Cost

GENBENCH (`HELIO_VOXEL_GEN_BENCH=6`), seed 75, RTX 3060, 1600x900, the
same holds before (old noise caves and lean, c610222d) and after:

| view | tiles us/tile | of which old halos / new volumes | chunk columns us/chunk | all us/tile |
|---|---|---|---|---|
| Earth cave, before | 25.36 | 0.73 | 9.93 | 40.56 |
| Earth cave, after | 23.40 | 2.44 | 6.58 | 34.25 |
| Earth mountain slope, before | 30.10 | 2.91 | 7.04 | 41.77 |
| Earth mountain slope, after | 24.75 | 2.41 | 5.34 | 34.16 |
| Desert cave, before | 21.47 | 5.29 | 17.90 | 45.84 |
| Desert cave, after | 16.55 | 1.76 | 4.49 | 24.79 |
| Desert mountain slope, after | 21.42 | 2.12 | 4.70 | 29.96 |

Generation is 15-46% cheaper per tile everywhere measured. The new tile
passes (`tile_caves`, `tile_volumes`) cost 1.8-2.4 us a tile, against the
old halo pass's 0.7-5.3. Chunk columns fall most (desert caves 17.9 ->
4.5 us a chunk): a column tests the few primitives its 8x8 brick kept,
not 3D noise at every cell of a depth band, and the band is the measured
volume range rather than the worst case. The "before" and "after" cave
holds stand at different places (the search follows the system), so
their rows compare generation over the same kind of view, not the same
pixels.

The Moon ground (no cliffs, lava tubes only where a network runs) is the
one place the volumes cost more, measured interleaved before/after, two
rounds each (the clocks moved between rounds: the second "before" was
slower in every stage, nodes and chunk builds included):

| Moon ground | tiles us/tile | of which halos / volumes | chunk columns us/chunk | all us/tile |
|---|---|---|---|---|
| before, round 1 | 30.76 | 0.06 | 2.50 | 36.85 |
| after, round 1 | 32.76 | 2.45 | 2.75 | 39.17 |
| before, round 2 | 39.66 | 0.06 | 3.24 | 47.29 |
| after, round 2 | 33.18 | 2.45 | 2.77 | 38.67 |

At matched clocks the columns are unchanged (28.7-29.1 us) and the tile
passes add their fixed 2.4 us: about 6% of the Moon's generation. The
old halo pass did next to nothing there (the Moon had no noise caves);
the new passes run on every tile within the volume levels. Skipping
tiles whose regions hold no network and no cliff gate would remove it.

Frame cost is unchanged in kind: the volumes only change which cells
are solid. Under the ground the first trace round walks from the eye
(`point_walk`), one extra walk for the pixels of an eye in a cave and
none above the ground.

## Open work

- Planet-map gates: karst share from rock type and drainage, lava tubes
  from volcanism (`terrain_cave_gate`).
- Cave floor materials through the per-cell material path.
- Aquifers (water levels in caves) once the engine has fluids.
- Structural integrity for floaters left by edits and ridge-crest ledges.
- Rock formations (arches, hoodoos) as feature forms in `flora`.
