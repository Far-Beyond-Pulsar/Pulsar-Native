# Camera lens

A physically based screen-space lens: focus field and thin-lens defocus, pupil
shape, a ray map (projection, distortion, lateral colour), transmission
(vignetting, filters), in-focus aberrations, diffusion filters and contaminants,
and real baked lens prescriptions. The optical model, every parameter and the
validation live in `crates/renderer/helio/crates/passes/3d/helio-pass-lens/README.md`;
this file is the map of how the pieces fit together in Pulsar-Native.

## Where things live

| Crate | What |
|---|---|
| `helio-lens-optics` | CPU lens tracer and baker (`Prescription`, `LensBake`); example lenses in `data/*.json` |
| `helio-pass-lens` | `LensPass`, `lens.wgsl`, the row types (`LensComponent`, `LensElementComponent`), `PrescriptionBank`, masks, the CPU twin of the ray map (`ray_map`), the bundled resources (`bundled`) |
| `helio-component` (`components/camera_lens_component.rs`) | the authored `CameraLensComponent` / `LensElementProps` / presets, and `camera_lens_view_overscan` |
| `helio-default-passes` | places the pass and asks for the bundled resources |
| `helio` (`camera.rs`) | `Camera::with_overscan` |
| `engine_backend`, `pulsar_game` | register the GPU columns (`helio_bridge`) and apply the overscan to the frame camera |

Rule of the architecture: component types and their logic live in Helio.
`engine_backend` and the game hosts only call two Helio-side entry points (the
GPU column registration and `camera_lens_view_overscan`, both flagged as legacy
debt in place) and never match on a lens type.

## Graph placement

```text
FogComposite -> Transparent -> TSR / FXAA -> LensPass -> LensFlarePass -> PostProcess
```

The lens works in scene-linear radiance, before anything is metered, bloomed or
tone mapped (`add_scene_linear_chain` in `helio-default-passes`). It reads the AA
output (`tsr_color` / `fxaa_color` / `fogged_hdr`) and the scene depth, writes
`lens_color` and publishes `lens_resolved` (`ResolvedLens`, 88 bytes) for the
flare pass. The old DoF pass is gone.

## Components

The authored value is `CameraLensComponent` (a reflected world component, view
`view_id`, the editor viewport is 0). On every insert, write, removal and replay
it derives GPU rows through the SceneDB mirror, like the camera post-process:

- `camera_lens_sources` (`CameraLensSourceRow`, at the instance entity's row): the
  pass's `LensComponent` bit for bit. The environment join gates it (attached,
  instance enabled, owner current) and publishes it as `camera_lenses`.
- `lens_elements`: a block of `LENS_ELEMENT_SLOTS` (8) `LensElementComponent` rows
  per lens from a compact allocator (the pass addresses a lens's elements as
  `element_first .. + element_count`).

The pass picks the lowest enabled row of the frame's view. Lenses are not blended
by volumes.

## Element kinds

`LensElementProps` is a flat struct (the inspector shows registered scalars,
bools, enums and one `Vec` of it, not nested structs). `kind` selects the group of
fields read; `kind_fields` + `reads` decide which the inspector shows and a test
(`the_inspector_shows_exactly_the_fields_a_row_reads`) holds them to `to_row`.
New kinds are appended to `LensElementKind` (the saved index is the kind) and to
the pass's `element_kind` in the same order.

| Group | Kinds |
|---|---|
| focus | `Diopter`, `SweetSpot`, `LongitudinalCa` |
| pupil | `Pupil` (iris, squeeze, cat's eye, swirl, spherical aberration, onion rings, apodization, obstruction, aperture texture) |
| ray map | `Projection`, `BrownConrady`, `Division`, `AnamorphicDistortion`, `AnamorphicSqueeze`, `Shift`, `Keystone`, `FocusBreathing`, `Prism`, `LateralCa`, `FlatPort`, `DomePort` |
| transmission | `NaturalVignette`, `OpticalVignette`, `MechanicalVignette`, `NdFilter`, `GraduatedNd`, `ColorFilter`, `TStopLoss` |
| sensor image | `SphericalGlow`, `Coma`, `Astigmatism`, `CornerSoftness`, `Diffusion`, `ProMist`, `BlackProMist`, `Glimmerglass`, `StarFilter`, `Halation`, `WaterDrops`, `Smear`, `Fog`, `Dirt` |
| baked and user | `Prescription` (a bundled lens, `prescription_lens` + `psf_amount`), `User` (hooks `user_map` / `user_transmission` / `user_focus` / `user_spectral` and `user_p0 .. user_p15`) |

Every element has a region (`FullFrame`, `HalfPlane`, `Ellipse`, `Polygon`,
`RadialBand`, `Mask`) in image-height units about the frame centre; regions feather
by the aperture unless set.

## Presets

`LENS_PRESETS` is data over the same fields (`LensPreset`, `Custom` uses the
fields). Any other preset replaces the optics, tilt, curvature, iris and the
element stack when the lens is resolved; the `bake_preset` method copies one into
the fields. Append new presets to `LensPreset`. Today: standard and portrait
lenses, split diopters, tilt-shift, sweet spot, anamorphic, vintage swirl, 8 mm
fisheye, underwater flat port, Pro-Mist and Black Pro-Mist, rainy lens, star
filter, film halation, grad ND and **Double Gauss 50mm f/2 (traced)**, a
full-frame `Prescription` of the bundled double Gauss.

## Prescription bank

`PrescriptionBank::new(&device, &[&LensBake])` uploads baked lenses as one storage
buffer (registry key `lens_prescriptions`); an element names a layer in
`params[0]`. `LensPass::with_bundled_resources()` (called by the default graphs)
traces the three example lenses on a background thread (`BundledLens`: thin lens
50 f/2 = 0, Cooke triplet 50 f/5 = 1, double Gauss 50 f/2 = 2; append only) and
uploads the bank when done; until then a `PRESCRIPTION` element covers nothing.
`LensPrescriptionChoice` in the component is the same list and order. Bake time is
about 0.8 s optimized and 2.1 s unoptimized (the root workspace builds
`helio-lens-optics` optimized in dev), off the render thread.

## Masks and iris shapes

`lens_masks` (`MASKS`) is a `D2Array` of RGBA8: layer 0 (`WATER_DROP_MASK_LAYER`)
is a procedural water-drop layer (normals in red/green, footprint in alpha), what
the Rainy lens preset reads. `lens_aperture_shapes` (`APERTURE_SHAPES`) holds
`BundledAperture` layers (star, heart, triangle) for `Pupil` elements'
`aperture_texture_layer`. Both come from `with_bundled_resources()` and are
published before the passes run. A project with its own masks registers them
under the same keys instead.

## Which view a frame renders as

Every host builds its frame through `SceneRenderer::prepare_frame`, which takes the
`helio::Camera` default (`view_id` 0) and calls `camera_lens_view_overscan` with
`camera.view_id`. So the editor viewport, play-in-editor and the standalone game
window all render as **view 0**; XR eyes are 1 and 2. Consequences for play:

- A `CameraLensComponent` or `CameraPostProcessComponent` with `view_id` 0 applies in
  the game wherever it is attached. It is not tied to the camera that is the view: a
  lens on the possessed character's camera object works, and so does one on a
  level-settings object. The lens is read from the lowest enabled row of the view.
- Which object's `CameraComponent` is the view does not change the view id, so
  switching view target keeps the same lens and post-process; per-camera lenses
  (a different lens per `CameraComponent`) are not supported. The seam would be the
  host giving the frame's `Camera` a view id taken from the selected camera.

`CameraComponent` (projection, clip planes, priority; `helio-component`) is the
authored viewpoint of the game's frame, see "Camera and view target" in
`TRAITS.md`. It carries no GPU row and none of the lens's fields.

## Overscan

A lens whose ray map looks past the frame (fisheye, barrel distortion) needs the
camera to render a wider view or the corners clamp to the edge pixel.

1. `camera_lens_view_overscan(&world, view_id, aspect)` (helio-component) finds the
   view's lens, derives the pass rows, and computes `ray_map::applied_overscan`
   (the CPU twin of the GPU's reach, including `Prescription` elements through the
   bundled ray maps; capped at 2; 1 when the lens stays inside the frame, is off,
   or the world has no GPU mirror). It writes that factor into the lens's GPU row
   (`LensComponent::overscan`) and returns it. The lens's reach depends on the
   aspect, so the row is rewritten when the aspect changes.
2. The host widens the frame camera: `camera.with_overscan(factor)` scales the
   projection's `x`/`y` rows by `1 / factor`. The editor viewport
   (`engine_backend` `helio_renderer/renderer.rs`), play-in-editor
   (`pulsar_game/src/embed.rs`) and the standalone game
   (`pulsar_game/src/windowed_app.rs`) each call both once per frame, before the
   mirror flush. The voxel chunk pass draws with that widened frame camera.
3. The pass treats its input as the frame widened by the row's factor
   (`frame_uv` / `image_uv` in `lens.wgsl`): the composite samples the map's
   result at `s / R`, and every scene-space quantity (regions, focus field, pupil,
   circle of confusion size, autofocus point) uses the frame's coordinates.

The render keeps its pixel count, so the centre loses `1 / R` of its sharpness;
scaling the target by `R` would cost `R^2` and reallocate on every lens edit.
Known limits: a `User` ray map's reach is not in the CPU value; a renderer pair
that shares a world and a view id but not an aspect (editor viewport and
play-in-editor) take turns rewriting the row, correct only when each calls right
before its own frame; screen-space overlays drawn after the lens (gizmos, flare
sources) use the widened camera; mouse picking uses the unwidened projection.

## User WGSL hook

`USER` elements run WGSL the project compiles into the lens shader:
`helio_default_passes::build_default_graph_with_lens_user_with_context(ctx,
snippet, user_effects)` (or `LensPass::new_with_user`). The snippet may define
`user_ray_map(uv, params, t)`, `user_transmission(uv, params)` and
`user_focus(uv, params, s)`; a hook it omits is the identity. The element's hook
bits (`flags`) choose which run, its 16 `params` pass through untouched, and its
region weights the result. Without a snippet `USER` elements are inert and set no
stage flags. A user ray map's reach is not part of the overscan.

## Verifying

```text
cd crates/renderer/helio
cargo test -p helio-pass-lens -p helio-lens-optics -p helio-pass-flare
cargo test -p helio camera::tests
cd ../../..
cargo test -p helio_component --lib
cargo test -p engine_backend --test camera_lens_rows --test environment_join_rows
cargo test -p ui_level_editor --test render_acceptance -- --test-threads=1
```

The lens tests run the production WGSL on a GPU and fail rather than skip.
