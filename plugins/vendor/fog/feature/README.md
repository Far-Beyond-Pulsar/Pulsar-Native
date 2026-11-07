# Physical volumetric fog

The pass owns its SceneDB schemas, activity resolution, quality selection,
transient grids, light assignment, and history. Core only supplies the generic
camera and SceneDB buffer projection. No CPU scene scan or renderer reach-in is
needed. All modifications in this implementation stay inside this crate.

## SceneDB interface

| Component | GPU buffer key | Packed bytes |
| --- | --- | ---: |
| `GlobalFogComponent` | `global_fog_media` | 64 |
| `LocalFogVolumeComponent` | `local_fog_media` | 112 |
| `VolumetricFogSettingsComponent` | `volumetric_fog_settings` | 48 |
| `FogComponent` (legacy) | `fog_components` | 64 |

All rows have removal/despawn callbacks that clear the packed GPU row. Zeroed
rows are inert, including holes before a component's first entity index.

`GlobalFogComponent` has `enabled`, `mode` (0 uniform, 1 height, 2 smoke),
`extinction`, `height_falloff`, `height`, `anisotropy`, `albedo`, and `emission`.
One world unit is one metre. Extinction is absorption plus scattering in inverse
metres; albedo is the scattering fraction; emission is scene-linear radiance per
metre, independent of extinction. Media overlap additively. Anisotropy uses a
scattering-weighted effective HG phase. There is no arbitrary ambient source.

Build a local AABB with `LocalFogVolumeComponent::new(min, max, medium)` and
adjust `edge_fade` in metres. `set_medium` and `medium()` pack/unpack the medium;
the underlying array is opaque bytes, not authorable numeric floats. New world
media have no view-distance gate and no PP priority or camera-inside condition.

Settings match `view_id` against the u32 bits of `Camera.jitter_frame.w`.
`u32::MAX` is an all-view fallback; exact matches take precedence, then lowest
entity row wins ties. `active=0` means absent; `active=1, enabled=0` explicitly
disables rendering. Settings are `quality`, `max_distance`,
`light_max_distance`, `temporal_blend`, `history_rejection`, `light_samples`, and
`history_epoch`. Increment the epoch for an explicit camera cut. The pass also
rejects history when the incoming previous-camera matrix does not match its
last rendered camera, on view switches, grid resize, range/quality changes,
lighting edits, and significant local density/radiance changes.

Quality 0 lights 96x54x64 froxels; quality 1 lights 192x108x128. The footprint is
adjusted to the view aspect while staying within this budget. Both tiers share
the same maximum-resolution allocations and integration output. The economical
tier reduces lighting to one eighth as many samples, but does not reduce the
60.75 MiB maximum grid allocation at 16:9. GPU settings changes need no readback
or synchronous allocation. `light_samples=0` selects 16/24 samples for economical/
high quality; explicit counts are clamped to 8–32. Light-path optical depth through soft-edged media needs this many samples: with 4 the quadrature error forms diagonal creases and stripes.

## Producer and consumer contract

Declare/read `shadow_matrices`, `shadow_atlas`, and optional
`postprocess_uniforms`. Record all fog work on `ctx.graphics_cmds()`, after current
shadows, with `chain_transparent=false`. The pass declares these dependencies
for the graph's parallel scheduler as well.

Publish:

- `fog_accum`: sampled 3D RGBA16F view, RGB integrated in-scattering, A
  transmittance. Disabled/empty scenes write exactly `(0,0,0,1)` throughout.
- `fog_parameters`: 64-byte uniform buffer with `GpuFogUniforms` layout.
  `fog_max_distance` at byte 20 is the resolved range. Composite using the
  shared prelude's froxel mapping. The enabled/density fields describe only the
  legacy adapter; **do not gate compositing on them**. Native media work when
  legacy fog is disabled. Always composite `color * fog.a + fog.rgb`.

The consumer owns placement before bloom/exposure/optics. This crate does not
change PP shaders, central renderer code, or default graphs.

Legacy PP camera fog is copied from `GpuPostProcessUniforms::FOG_BLOCK_OFFSET` when
available. The first enabled `FogComponent` overrides that baseline. Bounded PP
volumes still inject spatial media. Their row's opaque head and tail are generated
from the `GpuPostProcessUniforms` layout, so the fog block can move with PP changes. Native media need no PP buffer at all. Legacy authored distance
gates remain adapter behavior; native medium data has no such gates.

## Lighting, performance, and limits

The 128-byte light ABI is unchanged. `god_rays_enabled` is participation;
`god_rays_weight` is the scattering gain. Density/exposure remain multiplicative
for legacy authoring; new lights should set both to one. `god_rays_decay` is now
geometric volumetric shadow strength: `mix(1, visibility, clamp(decay,0,1))`.
Medium absorption still applies when geometric shadows are disabled.

Point shadows use the producer's +X/-X/+Y/-Y/+Z/-Z face order and shared
`helio_shadow_project` helper. Directional/spot shadows retain the shared atlas
projection convention. Light-path attenuation includes all media: uniform and
height profiles are analytic, local paths are clipped to AABBs before sampling,
and smoke/boundary fades use bounded quadrature. Point/spot attenuation covers
the complete light segment; directional integration is truncated at
`light_max_distance`. Dense smoke remains a sampling approximation.

An 8x8x4 cluster computes a conservative world AABB and assigns intersecting
range spheres once. Directional lights are always assigned; spot cones are
evaluated during lighting. Each cluster stores 64 light indices. Overflow scans
the full compact light list; global lists over 256 lights or 64 media scan the
source rows. This preserves contributions at the cost of performance under
overflow. Classification still scans sparse capacity once per frame. Empty
scenes dispatch zero injection/culling groups, while integration clears output.

The stable segment integral uses its series expansion near zero optical depth.
The grids remain half precision; extreme radiance is clamped at 65504 and very
small coefficients can underflow. History rejection favors prompt response to
light/shadow changes over temporal smoothing. This is single scattering with an
effective HG phase, not multiple scattering. Sky/indirect illumination and
IES/gobo sampling are not added. Each pass instance handles one view (camera
slot zero); stereo views need separate pass instances/outputs.

## Validation

Run `cargo test -p helio-pass-volumetric-fog --offline` from the Helio workspace.
GPU tests require an adapter and fail rather than silently skipping. They run
production WGSL and the graph: ABI/layout checks, thin/dense integration limits,
vacuum and lights-off history, all point-shadow faces using the real matrix
producer, shadow-strength interpolation, native SceneDB sparse rows,
world-space sampling, overlap, medium transmittance, quality changes, removal,
despawn/reinsertion, list overflow, and legacy PP volume compatibility.

No frame-time speedup or final-scene visual quality claim is made without a
representative scene capture. The culling/tier reduction is implemented and
tested, not benchmarked against a complete application.
