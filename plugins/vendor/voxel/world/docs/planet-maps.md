# Planet maps: drainage, erosion, rivers, lakes and climate

The terrain's octaves are local: each column is noise at its own point.
Nothing in them knows where water goes, where the coast is or how far a
place lies from the sea, so a planet of them has no drainage basins,
rivers, lakes or climate. A stack's **planet maps** give it that context:
baked once per world from its seed and layers, deterministic, integer
data the CPU and the GPU sample with the same integer arithmetic
(`src/planet_maps.rs`, `shaders/planet_maps.wgsl`).

## What a stack bakes

- A `Climate` (`layers::Climate`) with `maps` bakes temperature and
  moisture over the planet; without, the climate of its `latitude_deg`
  holds everywhere (heights still cool it).
- A `Rivers` layer (`LayerKind::Rivers`) bakes the drainage, wears the
  layers before it in the stack by its erosion (`coverage`: 0 none) and
  carves its rivers and lakes into them.

Earth's preset has both. The Moon, the desert and flat worlds are
unmapped (no bake, no table); voxel_craft's 16 km world keeps one climate.

## Layout

Maps cover the planet as six **gnomonic** cube faces (one square over a
plane), `n` texels a side at about `texel_km` at face centres (Earth: 25
km, 510 a side), at most `MAX_MAP_TEXELS` (6 x 512²) in all: larger
worlds get larger texels, so a bake costs the same from the Moon's size
up. A lookup takes a domain point (any level's column, a
cell's volume point): its face is its largest component's, and its face
coordinate `u = (a . p) / (n . p)` comes from one Q30 reciprocal
(`landform::recip_q30`): no transcendental functions, the same bits on both
sides, the same map at every level and voxel size.

The program's table (`TerrainProgram::tables`, `terrain_tables`, binding
40, `array<vec4<u32>>`) holds:

1. **River index**: per face `(n + 4)^2` words, a ring of two texels from
   the neighbouring faces around each face (a plane's ring repeats its
   edge). A word is a record id + 1 (0: none) and the record's reach in
   texels (Q4, top byte).
2. **Erosion**: per face `(n + 1)^2` vertex words: how far the drainage
   cut the terrain down (half metres, low half) and the level its sediment
   fills up to (metres, high half). Faces share their edges' vertices
   (one value each), so its bilinear lookup is continuous across them.
3. **Drainage**: per face `(n + 4)^2` words in the river index's layout:
   the wet area draining through the texel (`log2(1 + km²)` in 1/2048) and
   its flags (lake, ocean, river, dry sink).
4. **Climate**: per face `(n/2 + 2)^2` words at half the resolution with a
   ring of one: sea-level annual mean temperature (1/128 °C, low half) and
   moisture (Q16, high half). Bilinear between texel centres; the rings
   make the faces' seams continuous.
5. **River segments**, eight words each: the node (a domain point on a
   16-unit lattice, jittered within the middle half of its texel), the
   valley's geometry (wall slope Q6, floodplain half-width in 8 m, channel
   half-width in 0.5 m, a lake flag), the unit direction to the downstream
   node (Q15) and the segment's length (64 units), the bed at the node
   (mm), its drop to the downstream node's bed and the channel's depth.

`LandformConstants::maps` (`TerrainConstants::maps`) is the header: the
flags, sizes and offsets, the plane's half width, an unmapped world's
uniform climate, the lapse rate and the snow temperature, the valleys'
shoulder rounding, the erosion and drainage grids and the sediment's
rounding.

## Bake

`planet_maps::bake`, once per world: `planet_maps::maps` keeps bakes per
process and on disk, by a fingerprint of the stack's constants, map
settings, grid and the bake's own source (`planet_maps::fingerprint`; a
changed bake never loads an old file). Files go to
`HELIO_PLANET_MAPS_CACHE` (empty: none), `planet_maps::set_disk_cache`,
else the user's cache directory (`helio/planet-maps`); they are written
whole and renamed, checksummed, and a damaged or foreign file is baked
again. The newest eight are kept.

1. **Coarse heights**: the mean (and highest) of 2x2 samples a texel of the
   field without its rivers (`height_parts` without maps), at the level
   that resolves wavelengths of half a texel.
2. **Climate** at half resolution:
   - temperature: Earth's zonal means between `equator_c` and `pole_c` by
     latitude (around `pole`; a plane's from `latitude_deg` at its origin,
     north along -Z), with a little noise;
   - moisture: the rain belts by latitude (wet tropics, dry subtropics,
     storm tracks, dry poles), times the way to the sea: marching against
     the prevailing wind (trades and polar easterlies, westerlies) to the
     first sea texel, and across the coast any way (Dijkstra); rain
     falls off with distance (`continentality_km`) and behind ground
     higher than the place (`rain_shadow_m`); noise; then the rivers add
     moisture to the ground along them.
3. **Erosion** (`erode`), eight steps over the coarse heights at the
   climate's resolution (a quarter of the texels; its change is
   interpolated back): route
   them (the drainage below), accumulate the wet area, then from the
   sources down every texel's stream has a capacity, the square root of
   its wet area times its slope (scaled by the layer's `coverage`): under
   it, the stream erodes half the shortfall (down to half its drop to the
   next texel); over it, it drops 15% of the excess. Lakes keep what
   reaches them up to their surface, dry basins all of it (playas), the
   sea up to 10 m under its surface; hillslopes then creep 5% toward their
   neighbours' mean. Ranges wear down along their drainage, basins fill
   into plains, sediment fans out where streams leave the ranges. What
   changed becomes the erosion grid: a cut where the terrain wore down, a
   fill level where sediment settled, and elsewhere one far under any
   ground, so sediment's margins ramp down over a texel. Sediment `d` thick
   on average fills the hollows first: over ground spread evenly within
   the unresolved relief `R` around the texel's mean, its level is `2
   sqrt(d R) - R` over the mean (all buried past `d = R`), so a thin
   deposit only floors the deepest hollows and a thick one makes a plain.
4. **Drainage**: priority flood (Barnes et al. 2014, with an epsilon of a
   millimetre) from the sea, a plane's edges and dry basins (local minima
   where moisture is under 0.22: endorheic sinks; wetter basins fill and
   spill: lakes). Every texel drains to its steepest lower neighbour (8
   neighbours, across cube edges by 3D position); discharge is the wet
   area (area times moisture) upstream, accumulated from the highest
   filled texel down. The worn heights are routed.
5. **Rivers**: every land texel whose discharge reaches the layer's source
   (`region_km`² of wet land) and every lake (filled 30 m or more) gets a
   segment to its downstream texel's node. Beds: the filled height less
   the incision (a lake's is its level), made non-increasing downstream.
   Floodplain and channel grow with the square root of the discharge,
   incision and depth with its fourth root. Every quantity is in metres
   and powers of a half, with only IEEE basic operations and square roots
   (sines by their series), in a fixed order: every machine bakes the same
   maps.

Earth (seed 7, texels of 25 km): see the measurements below.

## Worn terrain

Before carving, the `Rivers` layer wears what lies before it in the stack
(`planet_maps::erosion`, `terrain_erosion` in WGSL): the column's height
`h` becomes

    smooth_max(h + cut, fill)

with the bilinear cut and fill level at its domain point, rounded over 30
m. Sediment buries the ground's hollows under a level plain and ramps
down where it ends; hills and ranges rise through it. A column buried a
metre or more gets the surface word `ALLUVIUM` (0x81): Earthlike
materials draw dry fans of sand and gravel or wet meadows over soil and
silt there, rules see no gully. Every level samples the same cut and
fill, so it is LOD-consistent; the bounds add the deepest cut, the most
sediment and the steepest step between vertices.

## Carving

The `Rivers` layer cuts what lies before it in the stack
(`planet_maps::carve`, `map_carve` in WGSL). A column's lookup visits the
5x5 index window around its texel; a record whose reach (in texels, a
box test) cannot meet the column is skipped. A segment's surface at the
column is

    bed(t) + wall * max(d - floodplain, 0) - channel(d)

with `d` the 3D distance (domain units, `unit_q30`) to the segment
from the column's point swung by the valley's meander field (13 km
wavelength, a third of the floodplain's half-width), `t`
the position along it (beds interpolate from node to node), `channel(d)`
a parabola of the channel's depth over its half-width at the distance
from the point swung further by the channel's meanders (wavelength 3.3 km
for large channels, 820 m for small ones, blended between 32 and 96 m
half-widths; amplitude one and a half half-widths, kept on the
floodplain), so the channel winds across a floodplain that stays put; the walls' foot
curves over half the floodplain (at most 409 m), so slopes never jump
there. The column's height
is the smooth minimum (`landform::smooth_min`, the layer's rounding) of
its terrain and the lowest surface of the segments within reach. A union
of continuous surfaces is continuous: confluences merge, segments meet at
their nodes, faces' rings hold the same 3D segments.

A record's reach is where its walls clear the highest ground the terrain
may have near it (the texels' highest samples plus every octave the maps
do not resolve), at most 0.8 of a texel: the walls steepen where a valley
would otherwise reach past it, so that no segment of a texel outside the
window can touch a column (a segment's texels lie two texels or more from
the column's).

**Levels.** Every level carves the same valleys (point samples of the same
surface). The channel shows where it spans four of the level's cells and
fades out by two (coarser levels keep its floodplain), so coarse levels
are band-limited samples and stay at or above finer ones.

**Bounds.** The layer's height range drops by the deepest incision and
channel; its slope bound (walls, beds and channels together,
`PlanetMaps::valley_bounds`) adds to the bound margins (`check_field`
samples them).

**Surface word.** A column in a channel whose bed is its ground gets the
surface word `RIVER_BED` (0x80; erosion bytes stop at -126): Earthlike
materials draw gravel and sand over silt there, rules count it as the
deepest gully and may require or refuse it (`MaterialRule::river_bed`).

## Streams

The maps resolve continental rivers (25 km texels on Earth); creeks need
the drainage hundreds of times finer, too much to bake for a planet.
`streams` refines it per region, on demand (`Rivers` layer `octaves`:
levels, at most 3):

- **Stems.** Every land texel drains to a neighbour (the bake's routing);
  its stem runs from its node to the downstream texel's, its bed the
  texel's (incision grows with the discharge's fourth root under a source
  too). The maps' rivers and lakes carve their own texels' stems.
- **Levels.** Each level splits every cell of the level above into 4 x 4
  (Earth: 6.25 km, 1.56 km, 390 m), each with a node jittered within the
  middle half of the cell. A cell belongs to the parent whose stem passes
  nearest its node (the stems' first 60%, among the 3 x 3 parents around):
  basins follow the streams, not the grid.
- **Routing.** A parent's cells drain to its stem: a priority flood over
  their nodes' heights (the worn terrain, without rivers, band-limited to
  the level) from the cells within 0.6 of a cell of the stem (or the sea
  beside them); each cell to its steepest lower neighbour, on the stem to
  a confluence a little downstream of its foot. Discharge is the wet area
  gathered within the parent; beds never rise downstream and never sink
  under the stem's bed at the confluence: where the terrain is lower than
  that, the stream stays underground and its valley begins downstream.
- **Valleys.** A stream is carved like a small river (floodplain and
  channel with the square root of its discharge, incision and depth with
  its fourth root; its valley runs straight, its channel meanders on its
  floodplain, the bend noise made only where a channel may be), within a
  reach of a quarter of its cell:
  its walls steepen up to 1.5 rise over run to meet the terrain there, and
  past half the reach its cut eases out to nothing (smoothstep), so it
  never steps. Its cut is at most three incisions and the channel's
  depth. A point meets only its own cell's segment and the eight around
  it (nodes keep a quarter cell from their cells' edges); faces' edges add
  the neighbouring face's cells.
- **Levels of detail.** A stream level shows where its reach spans eight
  of a column's cells and fades out by four: coarse levels are
  band-limited samples of the same streams, never deeper than the finer
  ones, and far footprints need no fine streams' regions (farther, a
  creek's valley is a few pixels).

Every quantity is deterministic, so each level stays consistent with the
one above. The CPU makes a parent cell's records once (`Streams`, cached);
the GPU reads the same records from the program's **regions**
(`TerrainRegions`, `terrain_regions` binding 41): the engine's
`RegionPool` keeps every region a footprint's columns may read resident
while it generates (its cave networks' reference ground beside it too,
`VolumeLayer::ground_columns`: a hub on a missing stream would move its
caves), making the missing ones on the CPU (in parallel) and evicting the
longest unused. A region is a parent cell's 16 records (and
a texel's stem), 13 words each: node, target, beds, floodplain, channel and
depth, reach, cut and wall slope, and its box on its face (signed bytes of
thirty-seconds of a cell around its segment, grown by its reach and
swing): a column reads one word of most of the nine cells it looks at.

## Sampling

What other systems read of the maps (caves near water, lava tubes,
materials, gameplay), through the terrain contract
(`TerrainField::{climate, erosion, drainage}`; `terrain_climate`,
`terrain_erosion`, `terrain_drainage` in WGSL; defaults for fields
without maps):

- `climate(p, height)`: temperature (1/128 °C) and moisture (Q16);
- `erosion(p)`: the cut and the sediment's fill level (mm); the sediment
  under a column is how far the fill level lies over its ground before it;
- `drainage(p)`: the wet area draining through `p` (`log2(1 + km²)` in
  1/2048, bilinear), its texel's flags (`DRAINS_*`: lake, ocean, river,
  dry sink), and the nearest river or lake whose valley reaches `p`: the
  distance to its axis (domain units) and its bed there (mm).

## Climate in materials and features

`terrain_climate(p, height)` (`TerrainField::climate`): the sea-level
temperature less the lapse rate at `height`, and the moisture.

- **Earthlike**: dry lands where moisture is under 0.3 (sand over sandstone
  and clay strata); the snowline is where the lapse rate brings the
  sea-level temperature to `snow_c` (lower toward the poles, under the
  tropics' 4 km), moved by moisture and noise as before; its alpine rock
  band under it.
- **Rules**: moisture and temperature ranges at the cell.
- **Features**: a spawn's temperature and moisture ranges at its ground
  (with its height wander); Earth's woods need rain, broadleaves give way
  to conifers below 7.5 °C, and the tree line is at 0.5 °C, so forests
  follow the climate and tree lines fall toward the poles.

## Costs

Measured on the development machine (16 threads, shared with other
builds; `planet_maps::tests::bake_costs`, `streams::tests::stream_costs`,
`--ignored --nocapture`), the Earth stack at seed 7:

| world | texels | first bake | of which erosion | table | cache file (decode) |
|---|---|---|---|---|---|
| Moon-sized (1737 km) | 114k (25 km) | 1.1 s | 0.6 s | 1.8 MB | 3.3 MB (7 ms) |
| Earth (6371 km) | 1.56M (25 km) | 10.4 s | 3.9 s | 23.5 MB | 44 MB (80 ms) |
| 2x Earth (12742 km) | 1.57M (50 km) | 9.3 s | 3.4 s | 27.4 MB | 48 MB (114 ms) |

The texel budget keeps larger worlds at Earth's cost; a bake's working
set is about 160 bytes a texel (~250 MB at the budget). Later runs load the
disk cache instead of baking.

Streams on the CPU: a cold area's regions (44 for a level-0 footprint)
take ~13 ms, ~0.3 ms a region; a level-0 column's height costs ~27 us with
three stream levels against ~23 us without (warm caches). The GPU reads
regions from the pool (896 bytes each, 32768 kept: 29 MB).

Generation on the GPU (`voxel_flight` holds, `HELIO_VOXEL_GEN_BENCH=3`,
RTX 3060, interleaved runs, all of a tile's generation per tile):

| view | before the maps | maps (rivers, climate) | + erosion, streams (3 levels) | + stream boxes | + straight valleys, lazy meanders |
|---|---|---|---|---|---|
| ground: all / columns (us a tile) | 32.7-34.5 / 19.4-20.3 | 36.4-36.6 / 23.4-23.5 | 45.8-46.8 / 32.3-33.2 | 46.3-46.5 / 32.5-32.8 | 42.3-42.7 / 29.1-29.5 |
| mountain slope: all / columns | 38.2-38.7 / 20.6-20.8 | 42.7-43.2 / 25.7-26.1 | 57.4-68.3 / 39.3-47.3 | 59.4-59.7 / 40.3-40.4 | 50.8-51.9 / 33.7-33.9 |

The last two columns are after the merge of voxel volumes (one chunk
world, a region pool per volume) and of the edit pipeline. Stream boxes
(`in_box`, one word for most of the cells around a column) cost nothing
measurable: the streams' cost was their meander noise, nine noise calls
for every column a stream level reached. Valleys now run straight along
their segments and the channel's two bend fields are made only inside a
floodplain, which brings streams to +6-8 us a tile over the maps without
them (ground +16%, mountain slope +19%).

Settling the held views took the same frames with and without streams
once regions were asked for ahead (ground 553 frames, mountain slope 491).
