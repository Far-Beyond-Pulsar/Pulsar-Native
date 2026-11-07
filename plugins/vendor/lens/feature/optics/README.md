# helio-lens-optics

Pure-CPU physical lens model for the Helio lens pass. It traces real camera lens
prescriptions once and bakes lookup tables (plain structs of flat `Vec<f32>`)
that a GPU pass samples later. No GPU, no `glam`; dependencies are `serde`,
`serde_json` and `bytemuck` only.

The lens pass reduces every lens effect per pixel to four quantities. This crate
produces the exact version of each:

| Quantity | Bake |
|---|---|
| ray map (sensor -> scene direction, per wavelength) | `RayMapLut` |
| transmission (vignetting) | `VignettingLut` |
| pupil function / aperture shape | `PupilFootprints`, `PsfTable` |
| focus field | `FocusData`, `PsfTable` |

```rust
use helio_lens_optics::*;
let rx = examples::double_gauss();                    // or Prescription::from_json(..)
let bake = LensBake::bake(&rx, &BakeConfig::default())?; // ~0.5 s release, offline
let bytes = bake.ray_map.as_bytes();                  // &[u8], upload as a texture
```

## Consumers

`helio-pass-lens` uploads a `LensBake` with `PrescriptionBank::new` and drives the
ray map (`RayMapLut.tan_theta`), the transmission (`VignettingLut`), the cat's-eye
pupil (`PupilFootprints`), the focus field (`FocusData`: per-channel axial focus,
field curvature) and the in-focus point spread (`PsfTable` at each field's best
focus) from it; see its README for what each replaces. It reads from a bake:
`ray_map.data` plane 0, the whole `vignetting` and `footprints`, `focus.*` and the
`psf` records; the distortion plane is implied by `tan_theta` and the effective
focal length (`y / (tan (1 + distortion))`, the small-field slope of the reference
wavelength). The bake's wavelength samples must ascend, `vignetting.f_numbers` must
ascend, and analysis fields (`focus.field_r`, `psf.field_r`) must ascend from 0.
Footprint orientation: the pass uses `+x` as the radial direction on screen.

The three `data/*.json` examples are what the editor offers: `helio-pass-lens`
bakes them once per process with the default `BakeConfig` (about 0.8 s optimized,
2.1 s unoptimized, on a background thread) as its bundled prescription bank.

## Model

* Units: millimetres, nanometres, radians. Optical axis is **+z**, light travels
  towards +z, the first vertex is at z = 0, the sensor is at
  `Prescription::sensor_z()`. Object space is air.
* A `Prescription` is a sequential list of `Surface`s. Each has `radius`
  (positive = centre of curvature towards +z; `0` = flat, because JSON has no
  infinity), `thickness` to the next surface (for the last surface: to the
  sensor), `glass` (the medium *after* the surface), `semi_aperture` (clear
  aperture; rays outside are vignetted), optional `conic` and even-asphere
  `aspheric = [A4, A6, A8, A10, A12]`, `stop` (exactly one surface; its
  `semi_aperture` is the open iris), and optional `thin_focal_length` which turns
  the surface into an ideal paraxial lens (`tan' = tan - h/f`, exact rectilinear
  projection, no aberrations).
* `Glass`: `Air`, `Index(n)`, `Abbe { nd, vd }` (two-term Cauchy fitted to
  `nd` and `vd`), `Sellmeier { b, c }` (wavelength in um) and
  `Catalog("N-BK7")`. Catalog: N-BK7, N-SF6, N-LAK9, F2, N-SK16, FUSED-SILICA,
  AIR (Schott / Malitson Sellmeier data).
* JSON via `Prescription::from_json` / `to_json`. Glass serializes as `"Air"`,
  `{"Catalog":"N-SK16"}`, `{"Index":1.5}`, etc.
* `Tracer` binds a prescription to one wavelength (indices are resolved once, so
  `trace` does not allocate), an optional stop scale / f-number and clipping
  mode. Real vector refraction (Snell, TIR), exact conic intersection plus
  Newton refinement for aspheric terms, clipping against every clear aperture.
  `Tracer::aim` solves (2D Newton on the real trace) for the ray from a given
  field that crosses the stop at a given normalized stop position, so pupil
  aberration is handled. `chief_ray`, `tan_for_height` build on it.
* `Paraxial` (matrix method): EFL, BFL, entrance/exit pupil position and radius,
  f-number, image distance for a finite object. `Prescription::focused(distance,
  FocusMode::Sensor | Group { first, last }, wavelength)` refocuses by moving the
  sensor or sliding an internal group (the front vertex and sensor stay fixed;
  `first >= 1`).
* Field is a meridional point: the field point sits on the sensor +x axis and the
  lens is treated as rotationally symmetric. "Tangential" = radial (sensor x),
  "sagittal" = azimuthal (sensor y). Normalized field `r` = sensor radius /
  `sensor_half_diagonal_mm` (0 axis, 1 corner).
* A real lens inverts the image. `tan_theta` below is the object-space angle
  tangent of the chief ray that lands at that sensor radius, measured on the
  **upright (display) frame**: a pixel at normalized radius `r` and azimuth
  `phi` looks at scene direction `normalize(tan cos phi, tan sin phi, 1)`.
* Finite focus: `BakeConfig::focus_distance_mm = Some(d)` (used by
  `LensBake::bake`, which refocuses the sensor first) uses object points in a
  plane `d` mm in front of the first vertex; `tan_theta` is then
  `height / d`. `None` is infinity. The individual `bake_*` functions expect an
  already focused prescription.

## Bakes

All tables carry their dimensions and are indexed as below; every one has
`as_bytes()` (`bytemuck::cast_slice`) except `FocusData`. Wavelength samples and
RGB weights are `SpectralSampling`.

### `RayMapLut` (distortion + lateral chromatic aberration)

`data[(plane * n_wave + w) * n_field + i]`, `f32`; texture: width `n_field`,
height `2 * n_wave`, `R32Float`, row = `plane * n_wave + w`. Field sample
`r_i = i / (n_field - 1)`.

* plane 0: `tan_theta` (see above), per wavelength. Interpolate linearly in `r`.
* plane 1: `distortion = y / (s_ref * tan) - 1` where `y` is the sensor radius in
  mm and `s_ref` the small-field `y / tan` at the reference wavelength. Negative
  is barrel, positive pincushion; differences between wavelengths are lateral
  colour. At `i = 0` it is the limit for vanishing field (`EFL(w) / EFL(ref) - 1`).

Metadata: `wavelengths_nm`, `reference_wavelength_nm`, `sensor_half_diagonal_mm`,
`valid_field` (beyond it the lens cannot image and entries repeat the last valid
one).

### `VignettingLut` (relative illumination)

`data[j * n_field + i]` for `f_numbers[j]`; texture: width `n_field`, height
`n_fnum`, `R32Float`. Value = `cos^4(image-space chief angle)` times the fraction
of the stop disk whose aimed rays survive every clear aperture (divided by the
on-axis fraction). Exactly 1 on axis and non-increasing with `i` (a running
minimum removes sampling noise). Default f-numbers: the nominal one, then
standard stops above it. The stop radius is scaled by `nominal / f_number`.

### `PupilFootprints` (cat's-eye bokeh shape)

`data[(f * resolution + v) * resolution + u]`, coverage in `[0, 1]`; texture
`resolution x (resolution * n_fields)` or an array texture of `n_fields` layers.
Texel centre `(u, v)` = stop coordinates `x = -1 + 2 (u + 0.5) / res`, same for
`y`; the stop is the unit disk, **+x points radially towards the corner**.
`field_r[f]` gives the normalized fields, `f_number` the iris. Fields the lens
cannot image are all zero.

### `FocusData`

Plain vectors, mm along +z relative to the sensor plane (+ = behind it):
`axial_paraxial_mm[w]` / `axial_real_mm[w]` (on-axis best focus per wavelength,
paraxial and real-ray least squares; chromatic and spherical focus shift),
and per analysis field `field_r[i]`: `tangential_mm`, `sagittal_mm`, `best_mm`
(best focus of the radial spread, azimuthal spread and both; field curvature
and astigmatism, reference wavelength).

### `PsfTable` (compact geometric PSF)

`data[(i * n_defocus + k) * stride + c]`, `stride = PSF_FIXED_FIELDS (5) +
n_profile`. Field `field_r[i]`, defocus `defocus_mm[k]` (plane shifted along +z
from the sensor), reference wavelength, ray offsets measured from the chief ray
on that plane:
`c = 0` RMS radius about centroid, `1` RMS tangential, `2` RMS sagittal (the
astigmatism ellipse axes), `3` centroid shift along x from the chief ray
(coma/lateral shift, signed), `4` edge radius (max ray distance from the chief
ray), `5 + b` encircled energy within `(b + 1) / n_profile * edge` (last bin is
1; a late-rising profile is soap-bubble bokeh, an early-rising one is soft). A
zero record means no ray was transmitted.

### Spectral

`SpectralSampling::uniform(n)` gives bin-centred samples over 400-700 nm
(`n = 3`: 450/550/650). `weights[k][c]` is the contribution of sample `k` to
output channel `c`, from smooth positive Gaussian R/G/B sensitivities,
normalized so **`sum_k weights[k][c] == 1` for each channel**. Recombine with
`out_c = sum_k weights[k][c] * image_k_c` where `image_k` is the scene sampled
through the ray map of wavelength `k`; with no chromatic aberration the images
coincide and the result reproduces the scene exactly (white stays white).

## Examples (`data/*.json`, `examples::*`)

| File | Lens |
|---|---|
| `thin_lens_50mm_f2.json` | idealized thin lens, zero aberration |
| `cooke_triplet_50mm_f5.json` | Cooke triplet, N-SK16/F2/N-SK16, ~40 deg full field |
| `double_gauss_50mm_f2.json` | six-element double Gauss, N-SK16 crowns + F2 flints |

Sources: the radii and thicknesses are the commonly tabulated "Cooke 40 degree
field" and "Double Gauss 28 degree field" sample data (Zemax sample files, also
reproduced in lens-design textbooks such as Smith, *Modern Optical Engineering*)
as recalled by the author of this crate. **They were not checked against the
primary sources**; the traced behaviour is consistent with those designs (the
Gauss gives EFL 103 mm at 100 mm scale with ~20 um on-axis RMS, a flat field
and about -5% barrel distortion at the corner; the Cooke gives 45.8 mm unscaled). Both
were uniformly scaled to EFL 50 mm, the stop was set to the nominal f-number
and the sensor to the paraxial focus. Glasses are mapped to the nearest catalog
glass; element semi-apertures are not from the source and were chosen to give
plausible cat's-eye vignetting. Treat the data as representative lenses, not
as faithful reproductions of a specific product.

## Limits

* Rotationally symmetric only; no decentre/tilt, no full 2D ray map.
* Geometric optics: no diffraction, no coatings/ghosts (flare is handled by the
  flare pass), no polarization; transmission is geometric clipping plus cos^4.
* Natural illumination is cos^4 of the traced chief ray angle, not a full
  radiometric integration; the pupil is sampled uniformly in the stop, not the
  entrance pupil. Pupil aberration therefore shows up as shape, not as flux
  weighting.
* PSF tables are geometric (ray density), single wavelength.
* Image medium is assumed to be air (no cover glass beyond the last surface
  as a distinct tracer step; model a filter as a flat glass surface pair).
* No mirrors / reflections; TIR terminates the ray.
* Group focusing needs a fixed front element (`first >= 1`); unit focusing is
  `FocusMode::Sensor`.
* Debug builds are slow for default-resolution bakes; bake offline or in release.
