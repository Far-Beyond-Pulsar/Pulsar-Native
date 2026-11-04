# Hierarchical Light-Field Sampling

HLFS shades the deferred GBuffer using visibility-guided light sampling. Its
output is linear HDR; the render graph supplies tone mapping and antialiasing.

Current RT measurements, quality gates, cathedral captures, and reproduction commands
are in the [review validation report](tests/validation/review-20260919/README.md).

## Visibility mode

`HlfsConfig::mode` defaults to `HlfsMode::ScreenSpace`. Both `HlfsPass::new`
and the `performance()` / `compact()` quality presets retain this default. Quality presets and
`HlfsDebugMode` control sampling and diagnostic output independently of visibility.

```rust
use helio_pass_hlfs::{HlfsConfig, HlfsMode};

let config = HlfsConfig {
    mode: HlfsMode::ScreenSpace,
    ..HlfsConfig::default()
};
// Pass config to HlfsPass::with_config(), or apply it with set_config().
```

ScreenSpace is the quality-first, non-RT-compatible path: current-frame depth
pyramid tracing, coarse/fine light grids, dual reservoirs, energy confidence and
validated unrectified history reuse. Tracing misses use the existing shadow maps.
A TLAS or ray-query-capable device never changes the selected mode automatically.
Its execution time depends on tracing and reservoir settings; it does not promise
a strict gameplay frame budget.

`HlfsMode::RayTraced` uses Vulkan hardware ray queries over the current opaque
triangle TLAS. Unsupported devices and missing/stale acceleration data are errors;
it does not silently fall back to ScreenSpace. The shared pass still owns grids,
reservoirs, history, spatial filtering and composition. Masked materials and the
larger production acceptance suite remain outside the validated RT scope.

`HlfsConfig::ray_traced_presampled()` explicitly opts into the experimental
two-sample/two-candidate tier, shading at half the output width and height.
It builds 256 current-frame weighted proposals per coarse tile, samples a weighted
alias table mixed with uniform discovery, and uses reactive history clipping.
Glossy surfaces use four samples with sixteen independent candidates each.
They bypass shared tile proposals to avoid coherent errors on narrow highlights. A globally dominant emitter is split out and evaluated with
an additional full-resolution visibility query; this is extra work, not part of
the base two-sample budget. Reconstruction blends valid bilinear neighbours.
This changes the estimator and denoising; it is not an exact-output replacement
for `compact()`. The regular presets and defaults retain the original sampler.
Changing the proposal mode specializes the visibility pipelines and resets history.

See [retained validation evidence](https://github.com/Far-Beyond-Pulsar/Helio/blob/4d56b104a442382449f7f2563773313621511d8f/docs/validation/hlfs/rt-control/scenedb/report.md)
for historical synthetic 1440p measurements of the previous one-sample tier,
not measurements of the current candidate. These are direct-lighting GPU
costs, including per-frame TLAS work, not whole-frame or general-scene guarantees.
The cathedral capture accepts `HLFS_RT=1 HLFS_PRESAMPLED=1` to exercise this preset.

## SceneDB RT integration

The frontend owns `helio_components::ray_tracing::SceneDbRayTracing`; this
crate reads no World. After flushing the World's GPU mirror,
call `acceleration.prepare(&world)` and publish its TLAS and rows with
`renderer.set_ray_tracing_frame(acceleration.frame())` before each render. The
renderer provides the TLAS through `RenderEnvironment`; publication expires
after that frame.
The helper supports opaque, indexed, world-space `StaticObjectComponent` casters,
including off-screen objects. Material and mesh generations are validated.
Masked/custom materials and known unsupported geometry buffers are rejected
(`SceneDbRayTracing::partial` leaves them out instead, for consumers such as
the GI probes that tolerate a missing caster; ray-traced shadows should not use
it).
Other geometry producers require an explicit acceleration projection.

The current helper scans objects and hashes each referenced mesh once per frame
to detect in-place edits. This CPU cost is excluded from GPU pass timings. Light
sampling covers allocated SceneDB rows, including zeroed vacant slots, rather
than truncating the population at 256. Allocation epochs do not track in-place
light edits, so composite repair history reuse is conservatively disabled;
the regular temporal lighting filter still runs.

The workspace profiler is pinned to a published `Pulsar-Native` Git revision.
No profiler checkout outside this repository is required.

## ScreenSpace frame stages

1. A 64×64 screen tile cull builds coarse light lists. An 8×8 depth-aware cull
   refines those lists. Overflow falls back to the complete light population,
   so exceeding a list's capacity does not remove illumination.
2. Each shading pixel evaluates a bounded set of candidates. Separate visible
   and hidden reservoirs combine the previous tile's observed lights with
   stratified discovery candidates. Reservoir and group probabilities are corrected
   in the lighting estimate. The hidden selection budget is normally capped at
   20%, relaxed to 50% on disocclusion. Directional proxy weights are limited
   relative to local lights without limiting the resulting lighting energy.
3. At most four selected lights receive shadow evaluation per shading pixel.
   Duplicate selections reuse visibility. Shadow maps use the scene's existing
   resources. Parallel workgroup selection gathers up
   to 16 explicit IDs from 64 deduplication scratch slots, then sorts them for
   binary-search membership. Visibility is binary; partial shadow coverage does
   not reduce a visible light's guiding weight. A current-frame minimum-depth
   pyramid supports cell-based traversal with pixel-depth confirmation. Rays
   adapt to projected length within the configured world distance and a hard
   96-iteration budget; misses continue to the shadow map.
4. Separate demodulated diffuse and specular histories use geometry rejection,
   a 5×5 YCoCg variance clamp and distance-dependent history rejection. Geometry
   history packs octahedral normals, logarithmic depth, age and both signals'
   luminance second moments into eight bytes. Specular demodulation uses the
   engine's analytic EnvBRDF fit, including roughness and view angle.
5. A sparse geometry-aware spatial filter runs at shading resolution and reuses
   the raw-lighting buffer. At least 80% visible-energy coverage reduces its
   footprint when temporal variance permits. Exactly evaluated and stable
   low-variance pixels bypass filtering.
6. Full-resolution composition chooses one geometry-matching bilinear neighbor
   with STBN. Optional half-resolution shading visits each 2x2 block. Unmatched
   surfaces reuse geometry-validated history without color rectification when
   lighting is unchanged; new geometry or changed lights use bounded fresh samples.

CPU recording does not traverse scene lights. GPU culling scales with light
count; candidate and shadow budgets per pixel are bounded. Total GPU frame time
is therefore **not constant in light count**. Intermediate textures and tile
lists are resolution-dependent and are reused between frames. External bind
groups are rebuilt when their actual GPU resource handles change.

`HlfsConfig` controls sampling and history. `Reference` evaluates every light
without denoising; `Unfiltered` exposes stochastic lighting before denoising.
`Confidence` displays the 80% energy-coverage flag.
`HlfsConfig::performance()` uses four samples per 2x2 block.
`HlfsConfig::compact()` uses two samples per 2x2 block for lower sampling cost,
with more variance than four samples. Both pass the reduced-resolution energy,
light-change, camera-motion and thin-geometry regressions. Full-resolution
sampling remains the default. Output format is selected separately at pass
creation; use `preferred_output_format()` for packed HDR with either preset.
`preferred_output_format()` selects renderable R11G11B10 HDR or RGBA16F fallback.
`enable_timing()` and `timing_query()` expose six GPU stage durations.
`output_texture()` supports readback. `allocation_bytes()` reports requested
storage, excluding driver alignment and allocator overhead.

## Validation

Shader validation runs without a GPU:

```text
cargo test -p helio-pass-hlfs --lib
```

GPU regression tests are explicit so ordinary CI does not depend on a display
adapter. Set `HLFS_CAPTURE_DIR` to retain comparison images.

```text
cargo test -p helio-pass-hlfs --test gpu_hlfs -- --ignored --skip benchmark --nocapture --test-threads=1
cargo test -p helio-pass-hlfs --test gpu_hlfs benchmark_gpu_light_scaling -- --ignored --nocapture
cargo run -p examples --bin indoor_cathedral_hlfs -- --capture target/hlfs-captures
```

Set `HLFS_REFERENCE=1` for the cathedral's matching full-light reference captures.
Set `HLFS_PERFORMANCE=1` to capture the reduced-resolution preset.
Set `HLFS_SAMPLE_COUNT=2` together with `HLFS_PERFORMANCE=1` to reproduce the
two-sample compact capture. This override is for capture tools only.
Set `HLFS_FXAA=1` to exercise the FXAA graph variant.
Measured results and images remain available in the
[validation archive](https://github.com/Far-Beyond-Pulsar/Helio/tree/4d56b104a442382449f7f2563773313621511d8f/docs/validation/hlfs).

## Platform choices

The visible lists use bounded workgroup memory rather than requiring subgroup
operations. Point, spot and directional lights use the existing light layout;
there is no rectangular-area-light representation to attach quadrant masks or
barn-door culling to. ScreenSpace does not require ray-query device features.

Diffuse and specular use the R11G11B10 unsigned-float bit representation in
portable RG32Uint storage textures, one packed word per signal. Explicit decode avoids requiring native
R11G11B10 storage-image support. STBN stochastic rounding covers normal and
subnormal values. Packed RG32Uint metadata stores octahedral SNORM8 normals,
FP16 logarithmic depth, two unsigned E6M5 luminance second moments, age and energy confidence.
Six exponent bits cover squared HDR luminance without overflow. Linear first
moments are derived from each signal's RGB; spatial filtering uses relative
variance in the same linear domain. Raw signals and ping-pong
history total 40 bytes per shading pixel; the full-resolution output is separate.
At 1080p with RGBA16F output, full shading requests 109,380,120 bytes
(104.31 MiB), or 43,487,640 bytes (41.47 MiB) at half resolution. Native packed
HDR output saves another 8,294,400 bytes. The performance preset then requests
35,193,240 bytes (33.56 MiB / 35.19 decimal MB), including grids and both history
parities. The default keeps full shading resolution. The approximate 34 MB
proposal budget applies to reduced-resolution shading, not full shading.

Coarse and fine grids pack two 16-bit IDs per word. Populations above 65,535
use a full-width global fallback. Visible IDs remain 32-bit. A dedicated
small-population pipeline skips reservoir setup and shared sorting when every
light fits within the shadow-sample budget.

Very dim candidates are culled only when an exact unshadowed contribution bound
fits a population-divided error budget. Aggregate loss is bounded by 1e-5 per
pre-exposed channel and never exceeds 1e-5 after removing pre-exposure. Bright
specular responses and large populations of dim lights retain support. Failed
half-resolution reconstruction first validates reprojected geometry and light
generation before history reuse, then shades the actual surface when history
is unavailable. Neighbor ray reuse is not part of this pass.

The benchmark uses GPU timestamps surrounding HLFS on the render encoder.
It compares the stochastic path with the full-light reference at the same
resolution. The cathedral capture separately reports serialized CPU + GPU frame
latency, excluding image readback. That completed-frame latency includes CPU
submission and GPU waits; it is not an isolated GPU pass time or asynchronous
throughput measurement. The validation report also retains a matched capture
from the previous renderer.

The embedded scalar spatiotemporal blue-noise ranks are independently generated
by `scripts/generate_hlfs_noise.py`. NumPy is only needed to regenerate the asset;
the renderer has no runtime Python dependency and includes no third-party
noise texture.


## Experimental weighted temporal reuse

`HlfsConfig::temporal_resampling` is opt-in and requires presampled RayTraced mode with `sample_scale=2`. It retains one 16-byte weighted light reservoir per configured base sample and shading pixel in each of two history textures. At 1440p and two base samples these textures total 56.25 MiB. Glossy samples above the base count remain fresh. All selected lights still receive current-frame visibility queries, including RGB transmission; old shadow results are not reused.

The effective history count is capped at seven prior estimates plus one fresh estimate. Geometry reprojection and a two-word light-data/key fingerprint reject incompatible history. Any detected change to light position, direction, intensity, color, range, type, cone or shadow policy resets these proposals globally. This conservative version therefore does not claim a reuse benefit for continuously animated light sets. A small positive target floor retains proposal support across receiver changes; difficult disocclusion and extreme light distributions still need broader validation.

For the capture harness, add `HLFS_TEMPORAL_RIS=1` alongside `HLFS_RT=1` and `HLFS_PRESAMPLED=1`. Defaults and presets remain unchanged. [Measured results and limitations](tests/validation/technology-20260919/README.md) show modest noise reduction, with the overall visual and performance acceptance gates still incomplete.
