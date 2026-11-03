# Scene-linear lens response

`LensFlarePass` models light the camera's optics scatter, computed from the
image the lens actually receives. It has no light list, no sprite atlas, and
no per-light settings: anything bright in the frame — sun disks, emissive
meshes, specular glints, bright fog — produces a response in proportion to its
radiance, and anything occluded or attenuated by media produces less.

Lens flare does not need fog. God rays do, and they are not made here: shafts
come from `helio-pass-volumetric-fog` integrating scattered light through a
participating medium, composited by `FogCompositePass`. See
`docs/physical-light-effects-audit.md` in Pulsar-Native for the full rationale.

## Where the settings live

Lens settings are part of the shared post-process settings, not the light:

| Author-facing | Runtime row | Resolution |
| --- | --- | --- |
| `CameraPostProcessComponent` (editor) | `helio_pass_postprocess::CameraPostProcessComponent`, matched by `view_id` | Camera baseline |
| `PostProcessVolumeComponent` (editor) | `helio_pass_postprocess::PostProcessVolumeComponent` | Overrides by priority, weight and inward boundary fade, per override bit |

`PostProcessVolumeBlendPass` resolves both on the GPU into the
`postprocess_uniforms` buffer. This pass reads the 96-byte lens block at byte
432 (`GpuLensResponse`); the prefix is opaque here.

The camera's optics are not post-process settings. F-number, focal length,
sensor width and the iris (blades, rotation) come from the view's lens
(`helio_pass_lens::LensComponent`) through the `lens_resolved` buffer the lens
pass publishes, with a neutral 50 mm f/2.8, 36 mm, six-blade camera when the
graph has no lens pass.

Natural `cos⁴` falloff is the lens's too (a `NATURAL_VIGNETTE` element, published as
`ResolvedLens::natural_vignette`): the lens applies it to the image this pass
reads, and this pass applies the same amount to analytic light sources, which
are not in that image. There is no separate flare vignette setting.

| Field | Meaning |
| --- | --- |
| `enabled`, `intensity` | Master switch and linear gain (0–16) |
| `quality` | 0 economical, 1 high |
| `profile` | 0 spherical, 1 anamorphic (2:1 squeeze, long horizontal streaks) |
| `threshold`, `soft_knee` | Scene-linear luminance selector. 0/0 makes the response a linear operator on the image |
| `ghost_count`, `ghost_intensity` | Reflected ghost images (≤4 economical, ≤8 high) |
| `halo_intensity`, `glare_intensity`, `streak_intensity` | Ring, veiling glare around sources, horizontal streaks |
| `dispersion` | Wavelength-dependent offset of R and B |

## Optical model

1. **Extract** (`cs_extract`): a 4×4 box reduction of the scene-linear input to
   quarter resolution. Every input texel participates, so sub-pixel peaks
   survive. The threshold/knee selects bright light without tone mapping;
   the lens pass has already applied natural cos⁴ falloff to its image (the
   `NATURAL_VIGNETTE` element), so the extraction does not apply it again.
2. **Pyramid** (`cs_downsample`): four 2×2 mean reductions of the extracted
   light. Every level carries the same mean radiance, so sampling a coarser
   level prefilters without changing energy.
3. **Respond** (`cs_response`), summed into one FP16 image. Every kernel
   samples the pyramid trilinearly at the level whose texel matches its tap
   spacing, so a fixed tap count gives a continuous shape at any radius — no
   dotted rings or copies of the source.
   - **Glare**: a normalized six-blade pupil kernel whose radius follows the
     Airy first zero at 550 nm plus a surface-scatter term. Normalization means
     the f-number changes the shape, not the exposure.
   - **Ghosts**: reflected images with signed magnification (negative values
     mirror across the optical axis). Ghosts are strongly defocused, so each is
     the source convolved with a large six-sided pupil: small bright sources
     read as soft aperture-shaped disks. The inverse area Jacobian keeps
     energy calibrated as magnification changes. Off-image energy is
     discarded, not wrapped.
   - **Halo**: each pixel gathers from a fixed distance along its direction to
     the optical axis, so a source maps to a continuous arc about the axis,
     fading toward the corners.
   - **Streaks**: a triangular-weighted horizontal kernel, longer for
     anamorphic.
4. **Composite** (in `PostProcessPass::fs_uber`): added after metering, with
   the same exposure as the image and bloom, then tone mapped once. No
   internal tone curve or clamp, so highlight energy stays predictable.

The ghost table is a synthetic 50 mm / 36 mm / f2.8 reference lens with
bounded energy fractions, not a measured prescription. Component weights sum
to a few percent of the extracted light, so the response never adds more
energy than a real multi-coated lens would scatter.

## Graph contract

```text
FogCompositePass -> Transparent -> TSR/FXAA -> LensPass -> LensFlarePass -> PostProcessPass
                                   (tsr_color | fxaa_color | fogged_hdr)
```

- Reads `postprocess_uniforms` (≥528 bytes, UNIFORM), the optional
  `lens_resolved` optics (a `ResolvedLens` storage buffer) and the configured
  HDR input (`with_color_input`; the default graphs pass the lens pass's
  `lens_color`).
- Writes `lens_output`: Rgba16Float at a quarter of the input resolution.
- Records on the graphics encoder after its producers. The compute encoder is
  submitted before graphics and would sample the previous frame.
- Reading the AA output keeps screen-space optics out of temporal history and
  out of motion vectors; composing after metering avoids flare→exposure
  feedback.
- An indirect dispatch is sized on the GPU from the resolved settings, so a
  disabled lens costs one clear and a one-thread dispatch — no CPU readback.
  Missing or undersized inputs clear the output and return.

## Quality tiers

| | Economical | High |
| --- | --- | --- |
| Glare / streak taps | 8 | 24 |
| Ghosts (pupil taps each) | ≤4 (7) | ≤8 (19) |
| Halo radial bands | 4 | 8 |
| Resolution | ¼ × ¼, 5-level pyramid | ¼ × ¼, 5-level pyramid |

Both tiers use the same model and energy calibration, so switching tiers
changes smoothness, not brightness.

## Limits

- Sources outside the frame produce no response (no overscan yet).
- Ghost shapes are pupil-shaped blurs of the source image, not ray-traced
  per-element reflections; there is no angular/focal-length ghost variation.
- Spectral dispersion is three-sample (R/G/B), not per-wavelength.

## Validation

`cargo test -p helio-pass-flare` runs the production WGSL on a GPU (an adapter
is required; tests fail rather than skip): ABI offsets against the real
`GpuPostProcessUniforms`, optics taken from the resolved lens, disabled/unbound clearing, threshold rejection,
glare energy bound and aperture normalization, ghost reflection across the
optical axis, and anamorphic streak shape.
