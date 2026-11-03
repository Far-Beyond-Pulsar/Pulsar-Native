# Water optics integration status

Helio's ocean surface now combines the four-band FFT displacement with
roughness-aware Fresnel/GGX sun reflection and stochastic finite-facet glints.
The microfacet roughness includes both FFT slope variance and geometric normal
variation over the projected grid cell. Glints use a Poisson facet estimate,
spatial LOD blending, and temporal blending to avoid hard cell and frame
transitions.

Refraction validates displaced depth against foreground depth and fades invalid
samples back to the unshifted scene. Water-column transmission uses
Beer-Lambert extinction with view/sun inscatter. Crest scattering and shoreline
travel-time waves contribute to the surface shading and breaking foam.

Foam coverage combines Ultra-Water's three-band, depth-damped FFT Jacobian
source with independent shoreline contact and authored shoreline-break sources;
there is no extra normal-steepness crest source. The volume maps
`water_color.w` to Jacobian bias (default `0.05`), `extinction.w` to Jacobian
scale only (default `1.3`), and `reflection_refraction.w` to foam brightness
(default `0.85`; legacy zero lanes fall back to `0.85`). Contact foam and
shoreline breaker/lace coverage are not multiplied by the Jacobian scale. A
tileable 512² cellular lace/billow material is baked once with explicit mips;
source strength erodes it into
filaments and open water, and its normal/billow channels shade surviving foam.
The surface shader now advects a coherent low-frequency freshness field through
the global patch phase and attenuates older foam while the fine material breaks
it into filaments. This improves age presentation; a persistent advected foam
state is still not part of Helio's surface contract.

The rendered surface, underwater fog boundary/Snell window, underwater shafts,
and sparse surface queries now use one shared shore-surface WGSL helper. It
combines the same three geometric FFT bands, shore-depth damping, authored shore
wave displacement, and volume-local interaction height; the 2.1 m capillary
band remains slope/roughness only. Surface vertices choose LOD from their grid
cell size. Underwater queries use stable LOD 0 instead of pixel-derivative LOD,
so changing fullscreen footprints do not shimmer the interface; fog uses four
fixed-point steps to undo horizontal chop. Half-resolution shafts also use LOD
0 but skip that inversion because their 24-sample ray march evaluates many
receiver points per pixel. Their depth/caustic boundary therefore keeps the
same vertical field while accepting the small horizontal-chop approximation.

The 12 m FFT slope band generates repeatable area-ratio caustics from
refracted triangle splats. A cubic B-spline samples the slope field, 4x MSAA
preserves thin fold coverage, and additive splatting sums overlapping light;
sun below the horizon yields the neutral multiplier 1. The near target is
512² RGBA16Float with the 1.5 m slice in R. The deep target is 256² RGBA16Float
with the 4 m slice in R and 10 m slice in G. Each target owns a complete
repeatable mip chain: 10 levels for near and 9 for deep. Consumers choose LOD
from projected footprint using the corresponding target's texel scale. The
water surface, deferred receivers under the global ocean, and half-resolution
underwater shafts sample these same textures. Deferred and water consumers use
the world-origin phase reduced in f64 to the 12 m period; the legacy 270 m
volume phase is not reused. Underwater shafts march depth at half resolution,
include the caustics field for submerged samples, and use a four-tap
depth-aware upsample in the full-resolution medium composite.

Breaker spray uses a deterministic packed set of up to 150,000 emitters sampled
uniformly from authored shore-map texels where water depth is 0.3–3.2 m and
signed coast distance is at most 150 m, matching Ultra-Water's surf-zone
selection. Each emitter is jittered within its source texel and stores map-local
coordinates, so camera/world-origin rebases do not move it. CPU packing runs
only when the shore map or requested count changes. Its 16-byte rows use a
storage buffer whose count is capped by the device's maximum storage-binding
and buffer sizes; `effective_spray_emitter_count()` reports the bound count.
`WaterSpraySettings` controls enable, amount, size, max distance, and requested
emitter count (150,000 by default). Breaker spray stays inactive without an
authored shore map. The vertex shader uses the same two travel-time trains,
phase noise, breaking envelope, and horizontal/vertical shore displacement as
the rendered shore wave, then emits ballistic camera-facing droplets. Host impacts can call
`WaterSimPass::add_spray_impact` with an absolute-world `DVec3` on the water
interface, radius in metres, and relative energy; events rebase to the current
frame origin, age on the frame clock, and are capped at 16 (oldest dropped on
overflow). Both effects depth-test against opaque scene depth, use a soft
intersection fade, and blend after the water surface. They do not yet write
Ultra-Water's per-surface water tags, so droplets crossing the interface lack
its tag-aware underwater compositing behavior.

The legacy volume row supplies ocean height and optical parameters; its XZ
AABB does not cap the ocean, the caustics field, or the underwater surface
query. The row's local interaction heightfield remains confined to its authored
XZ bounds. Terrain-derived shoreline depth/travel-time is opt-in through
`WaterShoreMap`, since generic Helio scenes do not expose a terrain-height query.

## Remaining Ultra-Water differences

- Above-water reflection now uses a half-resolution mirrored opaque capture,
  reverse-Z oblique clip plane, and prefiltered mip chain. The capture owns a
  persistent secondary graph with mirrored GBuffer winding and deferred light.
  Its absolute rest plane defaults to world Y=0; call
  `WaterSimPass::set_ocean_rest_height` when the live water row uses another
  `bounds_max.w`. Underwater frames invalidate the capture so the surface uses
  sky/SSR rather than a stale reflected view.
- The underwater medium still lacks Ultra-Water's mirrored Snell-window capture,
  medium-specific surface tags/meniscus, and view-dependent underwater
  reflection treatment. Both surface rendering and the underwater fullscreen
  medium evaluate a local FFT-slope Snell window with total-internal-reflection
  behavior. The fullscreen pass still uses analytic sky above the interface.
- Caustics use the existing global 12 m FFT slope band and fixed three-depth
  slices. Terrain-accurate receiver depth is not available without a height
  source; the terrain shoreline field remains separately authored and opt-in.
- Beach swash/run-up now shades top-facing voxel terrain from the shared shore
  travel/front fields when the host enables the voxel scene-material bridge;
  it is not active in the legacy voxel material-table path. Persistent
  advected shore foam, marine snow, and surfacing lens droplets remain to be
  ported.
- Sparse surface queries and buoyancy/drag calculations exist, but no current
  physics runtime submits floater points or applies forces; the `pulsar_physics`
  crate currently registers component data without a simulation backend.
