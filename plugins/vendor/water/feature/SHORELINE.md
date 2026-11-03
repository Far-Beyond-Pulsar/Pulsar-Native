# Shoreline input contract

The water pass renders one camera-centered continuous ocean. Shore waves are
opt-in. It never infers coastlines from water-volume AABBs. The first live
`WaterVolumeComponent` row supplies ocean rest height and optical settings;
existing `wave_params` remain for localized legacy interactions and do not
modify the four authored FFT spectra.

`WaterSimPass::set_shore_map(Some(map))` uploads a complete terrain-derived
512×512 map and adopts its absolute rest height for planar reflection. Keep
that height equal to `bounds_max.w` on the live water volume rows. Each field
is row-major RGBA32F:

| Field | R | G | B | A |
|---|---|---|---|---|
| `shore` | Terrain elevation relative to ocean rest plane (m) | Signed coast distance (m; positive in water) | Coast-distance gradient X | Coast-distance gradient Z |
| `travel` | Signed shallow-water travel time (s) | Direction toward shore X | Direction toward shore Z | Shoaling factor |

Both maps share an absolute-world XZ center and full XZ extent. The pass
converts the center to its camera-relative frame in f64, uploads maps only when
they change, and bilinearly samples them. Out-of-map points are open ocean.
`set_shore_map(None)` disables shore waves and leaves the FFT ocean active.

Breaker spray emitters are deterministically packed from the same authored
map. Candidate texels use Ultra-Water's depth interval (0.3–3.2 m) and maximum
150 m positive coast distance; accepted emitter positions receive an
independent jitter within the source texel. `WaterSpraySettings::emitter_count`
requests 150,000 by default and is capped at that count. Valid texels are
sampled uniformly with replacement, so narrow surf zones still get the full
requested density when they contain eligible map samples. Rows store XZ
offsets from the map center and stay stable through camera-origin rebases. The
GPU buffer is rebuilt only after a map or requested-count change, and its row
count is reduced when the device's storage-binding or buffer-size limits
require it; `WaterSimPass::effective_spray_emitter_count()` returns the bound
count. An empty surf zone binds one inert dummy row and draws no breakers.

The water pass publishes `water_shore_fields` and `water_shore_params` for the
voxel terrain material bridge. `ChunkPass` declares both as graph reads, so its
terrain draw is ordered after the field producer. The bridge samples the same
shore elevation, coast direction, and eikonal travel direction to animate a
land-side run-up film, foam edge, wet darkening, and roughness change. Swash
uses the same clock and travel-time/front fields as the water; its spatial
phase noise is anchored relative to the authored map center so world-origin
rebases do not move the wet line. The terrain effect is consumed when
`ChunkPass::set_scene_materials` uses `chunks::scene::helio_scene_materials`
on a device with texture binding arrays; without that optional material path,
the water resources remain available but voxel shading does not evaluate the
swash yet. The swash currently uses Ultra-Water's default 0.55 m amplitude,
30 m wavelength, and 1.0 speed.

`WaterShoreMap::from_terrain` samples every texel from a typed
`WaterTerrainProvider` and fails if any elevation is unavailable. It computes
an exact separable Euclidean coast-distance transform with light blur, floods
land islets smaller than 64 texels, then solves the shallow-water eikonal field
with fast sweeping and `sqrt(g * clamp(depth, 0.3, 14))` speed. Front direction
and coherence, curvature, Green's-law shoaling and ray-tube convergence follow
Ultra-Water's `shoreMap.js` pipeline.

Water names no terrain. Helio's voxel terrain's provider is
`helio_presets::water_shore::VoxelShoreTerrain` (and
`shore_map_from_voxel_planet`, which bakes with it), where the two features
meet: it samples the voxel world's canonical `Matter::Ground` column query,
including terrain edits. For sphere-shaped planets, it follows the planet's
radial ground column under each ocean-plane sample; for voxel planes, this is
the exact vertical column. Keep spherical-planet maps to a local tangent patch
because the rendered ocean uses world XZ coordinates. Install the result with
`WaterSimPass::set_shore_map`. The scene author chooses the ocean rest
height and map domain; baking is a one-time CPU authoring operation.
