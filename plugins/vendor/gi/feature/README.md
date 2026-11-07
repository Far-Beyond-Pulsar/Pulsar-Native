# helio-pass-gi

World-space diffuse global illumination: a camera-centred clipmap of
irradiance probes kept up to date by rays every frame, read by deferred
lighting at every pixel (meshes and voxel terrain alike). It gives bounce
light from sunlit ground into shadow, sky light that does not leak into caves
or under overhangs, and light that changes as the world is dug and built.

## Probes

- **Clipmap.** `LEVELS` (6) cubic grids of `SIDE`^3 (24^3) probes, each
  twice as far apart as the one inside it. The finest spacing is the
  pass's (`GiConfig::finest_spacing`, the `gi` settings group's `probe_spacing`):
  1 m over voxel terrain (1, 2, 4 .. 32 m apart: half-extents of 12 to
  384 m around the eye), 0.5 m in mesh scenes, whose rooms, pillars and
  furniture are a few metres apart (6 to 192 m). Probes sit at whole
  multiples of their spacing in world coordinates, and are stored
  toroidally: storage coordinates are world coordinates modulo `SIDE`. A
  probe keeps its record while it stays in the clipmap, and a grid that
  scrolls resets only the slabs that came in. The clipmap is a function of
  the eye's world position and the finest spacing alone
  (`Clipmap::around`), published each frame as `gi_clipmap`, so a tracer
  keeping state per probe follows it.
- **A probe** (`gi_probes`, 72 words) holds:
  - its irradiance / pi as L1 spherical harmonics (RGB, half floats);
  - its state (active, updates taken);
  - its visibility as an 8x8 octahedral map of ray distance moments (mean,
    mean^2; half floats), as in DDGI.

  A probe whose rays mostly meet back faces (a quarter or more) is inside
  the ground, a wall or a solid mesh: inactive, so it takes no weight.
  A probe on a mesh's surface (a hit within millimetres of it: probes on
  a floor at a whole multiple of the spacing) counts as inside, so all of
  them are dropped alike rather than some, as f32 rounding would pick.
  Probes are not moved out of geometry (DDGI's relocation): a probe
  inside is dropped, and the visibility test keeps its neighbours from
  leaking through.
- **Updates.** Each frame updates `LEVEL_UPDATES` probes per level (680
  on the two finest, 340 on the coarser, whose far surroundings change
  slowly and whose rays walk farthest): those that came in first, oldest
  first, then the others in turn. Each
  update traces 64 rays along a spherical Fibonacci set turned by a random
  rotation each frame. Results blend into the probe with hysteresis (1/n
  for its first updates, then 3% for irradiance and 5% for moments).

## The frame

`GiPass` runs before the tracers:

1. **`gi_sun_main`:** the sun. With an atmosphere, its direction and its
   illuminance through the air to the eye; without one, the scene's first
   directional light.
2. **`gi_integrate`:** last frame's rays, a workgroup per probe, a lane per
   ray. A ray that hit lights its probe with radiance `albedo * (sun *
   visible * clouds * cos / pi + probe irradiance at the hit) + gi_light`, where
   `clouds` is the share of the sun the clouds let through to the hit
   (the frame's `sun_transmittance`, the sky's clouds publish it): without it, ground under a cloud was lit
   by bounce from ground the cloud shades, as bright as in the sun, and the
   shade read flat. The probes' own
   irradiance at the hit gives every further bounce. A back face gives
   nothing. A miss sees the sky: the atmosphere's sky table at the probe,
   cosine-convolved L2 harmonics turned back into radiance band by band.
   Without an atmosphere, the flat ambient above the horizon and 0.15 of it
   below. Once a probe holds light, a ray's radiance is capped at 16
   times the probe's mean irradiance / pi plus 2: a ray meeting a surface
   centimetres from a point light (its fitting) would otherwise dominate
   its probe for many updates (fireflies).
3. **`gi_reset`:** the probes that came in.
4. **`gi_rays_main`:** this frame's rays (`gi_rays`).

It then clears `gi_hits` and `gi_light` and publishes `gi_frame`,
`gi_probes`, `gi_rays`, `gi_hits`, `gi_sun`, `gi_light` and `gi_clipmap`.
A pass whose only tracer is the meshes' (`GiConfig::needs_tlas`: no voxel
terrain) runs only on frames with a TLAS and publishes nothing on the
others, so deferred lighting keeps the sky's ambient.

**Tracers** read every ray and write the hits of their own geometry
(`GiHit`: distance, packed normal, albedo RGB8 with the sun's visibility in
alpha, and the tracer), and any light the hit gives off beyond the sun's
and the probes' (`gi_light`, RGB half floats, added as is). Tracers:

- **Voxel terrain** (`helio-pass-voxel-planet`, `gi_trace`): each probe's
  base cell is located on the CPU in f64 when the probe comes in
  (`ChunkWorld::follow_gi`). A ray walks the drawn cut from there: its
  chunk; the air box it is in, in the frame of a drawn chunk beside it; or
  rock (a back face at once). Its hit walks to the sun from half a cell off
  the face. Probe rays see the cut as the camera drew it: coarser where the
  camera needs less detail. The test `terrain_traces_gi_probe_rays` checks
  them against CPU rays.
- **Meshes** (`GiMeshTracePass`, `gi_mesh_trace.wgsl`), after the others,
  on a device with hardware ray queries: a ray queries the frame's TLAS
  (`RenderEnvironment::tlas`) up to the hit already written and keeps the
  nearer. The hit is shaded without a G-buffer from its instance's row
  (`helio_core::RayInstance`, `ray_instances`: the material's base colour
  and emission, where its triangles are in the scene's mesh pools): the
  triangle's normal, turned to the side its vertex normals call outside,
  and their interpolation; a hit on the inside is a back face. The sun's
  visibility is a shadow ray through the TLAS times the sun's occlusion
  maps (terrain, when published) and the clouds' shadow. Whichever hit stands (the terrain's
  too), if it faces the ray, then takes the meshes' shadow of the sun and
  the light of one of the scene's point and spot lights: chosen in
  proportion to its unshadowed irradiance there, with a shadow ray, over
  its chance (`gi_light`). The frontend publishes the TLAS and its rows
  (`Renderer::set_ray_tracing_frame`, from
  `helio_components::ray_tracing::SceneDbRayTracing`; `partial`
  leaves out what it cannot represent, such as engine foliage layers, so
  the probes see past it). Textures are not sampled for shading: the
  projection folds each base colour texture's mean (its coarsest mip,
  measured once on the GPU) into the row, so a textured material whose
  factor is white does not bounce as white. Local lights' shadow rays see
  meshes only, not voxel terrain.

  Cut-outs (alpha-tested materials: leaf cards, fences) are non-opaque in
  the TLAS, their rows flagged `RAY_INSTANCE_MASKED` with their material
  row. Probe and shadow rays test each candidate as the raster does
  (`helio_mats::vg_alpha`: base colour alpha times the factor against the
  cutoff, uv0 by the barycentrics), sampled at the mip whose texels match
  the ray's footprint there (a cone of one probe ray's solid angle). Past
  the finest mip the test is stochastic (alpha against a uniform
  threshold), so a canopy stops rays in proportion to its coverage however
  far, instead of its thin leaves vanishing with the mips; at most
  `MAX_CUT_OUTS` candidates are tested a ray, the next one stops it. A hit
  on a cut-out is shaded as any mesh hit; a double-sided material's faces
  are both outside.

  Virtual geometry is traced through one coarse proxy cut per asset
  (`helio_components::ray_proxies`, rows flagged `RAY_INSTANCE_PROXY`:
  positions and uv0 in the proxy pool, the face's winding as its normal).

  Without ray queries (software Vulkan, WebGPU) the tracer is not built:
  over voxel terrain the probes see the terrain alone (meshes receive GI
  but neither occlude nor bounce it); a mesh scene has no GI, the sky's
  ambient as before.

`GiResolvePass` runs after the tracers and before deferred lighting. For
each pixel it writes `gi_irradiance` (`gi_sample.wgsl`): irradiance / pi
and how much the probes hold it. It reads the finest level whose grid holds
the point. The 8 probes around the point are weighted by:

- trilinear position;
- facing;
- a Chebyshev test of the point's distance against each probe's moments
  toward it.

Near its grid's edge, the level blends into the next one.

## Costs (targets, 1080p, RTX 3060 class)

2720 probe updates a frame (174k rays). With the terrain tracer about
2 ms (its sun from the occlusion maps). The mesh tracer: a ray query a
ray, a shadow ray to the sun from hits facing it and one to a chosen
light. Memory: 6 x 24^3 probes x 288 bytes (24 MB), plus rays, hits and
their light (9.8 MB).
