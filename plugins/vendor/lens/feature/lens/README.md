# Physically based screen-space lens

`LensPass` is the camera's lens: it applies what the optics do to the image the
sensor would record, in scene-linear radiance, before anything is metered,
bloomed or tone mapped. It models the **focus field**, the **thin-lens
defocus** it implies, the **pupil function** (what that defocus looks like)
with longitudinal chromatic aberration, the **ray map** (projection,
distortion, spectral lateral chromatic aberration) and the **transmission**
(vignetting, filters, T-stop), then the **in-focus aberrations, diffusion
filters, stars, halation and contaminants** of the sensor image. The old depth of field (`helio-pass-dof`) ran on the tone
mapped image and had no optics behind it; it is gone, and so are its
post-process settings.

## The model

A lens is an ordered stack of optical elements. The stack reduces, per pixel,
to four quantities, followed by a post stage on the sensor image:

| Quantity | Now |
| --- | --- |
| Ray map (sensor uv to scene uv, per wavelength) | composed in stack order from the projection, distortion, shift, keystone, breathing, prism, lateral CA and port elements |
| Focus field `s(uv)` | resolved: base focus, tilt, field curvature, diopters, per-channel focus shift |
| Pupil function | `pupil_shape` and `pupil_profile`: iris, squeeze, cat's eye, swirl, aberration, apodization, obstruction |
| Transmission | natural cos⁴, optical and mechanical vignetting, ND / graduated ND / colour filters, T-stop loss, diopter seam darkening |
| Sensor-image aberrations and filters | field-dependent PSFs (glow, coma, astigmatism, corner softness), diffusion (Pro-Mist, Black Pro-Mist, Glimmerglass, fog), stars, halation, water drops, smear, dirt |

All of the above can instead come from a baked real lens (`PRESCRIPTION`, see
"Baked prescriptions") and the host can add its own hooks (`USER`, see "User
elements"); the shader dispatches on `LensElementComponent::kind`.

### Focus field

Focus is carried in dioptres so elements add:

```text
1/s' = 1/s + sum(D)
```

- **Base focus** is `focus_distance_m`, or the autofocus position.
- **Tilt** (Scheimpflug) tilts the focal plane by `tilt_angle` about an image
  plane axis at `tilt_axis`: the ray through a pixel meets it at
  `cos(a) s0 / (cos(a) + sin(a) (axis . d))`.
- **Field curvature** (Petzval) adds `field_curvature * r^2` dioptres, `r` the
  distance from the centre, 1 at the corner.
- **Diopter** elements bring `params[0]` metres into focus where their region
  covers the pixel. The filter's power is `D = 1/params[0] - 1/s`, taken
  against this frame's focus `s` (after autofocus and tilt), so a focus pull
  leaves the region sharp at its distance. It is a focus, not a blur: each
  covered pixel's circle still comes from its own depth, so only geometry at
  the diopter's distance is sharp there and nearer or farther geometry blurs
  by the thin-lens amount. Any number stack: two-way and three-way splits,
  nested regions, any region shape. Where several cover a pixel the last one
  sets the focus, as the filter nearest the lens would.
- **Sweet spot** elements (Lensbaby) leave focus normal inside their region and
  add `params[0]` pixels of defocus outside it, in quadrature with the thin-lens
  circle.

### Defocus

The signed circle of confusion, a diameter in input pixels, negative in front
of the focal surface:

```text
c = A |s2 - s1| / s2 * f / (s1 - f),   A = f / N
```

`s1` is the pixel's focus distance from the field, `s2` the scene depth along the
optical axis, converted through `sensor_width_mm` to pixels. Sky is at infinity.

### Pupil function

Each source spreads its light over its circle of confusion, and the pupil
function says how: `P(rho, theta; field position, defocus sign, wavelength)`,
evaluated per gather sample in the pupil's own frame (the offset divided by the
source's circle radius). It splits into a **shape** (where light passes, 0..1)
and a **profile** (how bright it is across the disc). Every shape lies inside
the unit circle, so the tile search radii of the circle of confusion stay valid.

Where it lives: lens-wide `aperture_blades` / `aperture_rotation` on
`LensComponent` give the base iris (a straight polygon, or a circle at 0
blades). Everything else is a `PUPIL` **element**, so it can be limited to a
region and different screen areas can have different bokeh. Where several
`PUPIL` elements cover a pixel the last one wins; there is no blending across a
region edge (the region is tested at 50% coverage). `LensComponent` is unchanged.

| `PUPIL` params | Effect |
| --- | --- |
| `BLADES` `CURVATURE` `ROTATION` | Polygon iris, 3..16 blades (0 circle; negative inherits the lens's blades and rotation). Curvature 0 is straight blades, 1 a circle |
| `ROUND_UNTIL_F` | f-number at which the blades are as curved as `CURVATURE` says: a circle wide open (below half of it), blending up to it (wide open round, stopped down polygonal). 0 disables |
| `TEXTURE_LAYER` `TEXTURE_AMOUNT` | A layer of the aperture texture array (coverage in red, the square around the pupil circle, masked by the circle) blended over the polygon |
| `SQUEEZE` | Anamorphic: the pupil is this many times taller than wide (1.33, 1.5, 1.8, 2.0) |
| `CATS_EYE` `BARREL_RADIUS` | Optical vignetting: the iris is clipped by a second circle (radius `BARREL_RADIUS`, default 1) displaced toward the optical axis by `CATS_EYE * r` pupil radii, `r` the field radius (1 at the corner). Round on the axis, lens-shaped toward the corners |
| `SWIRL` | The disc is squashed radially by `1 + SWIRL * r`: tangential stretch growing with the field radius (Petzval, Helios) |
| `SPHERICAL` `SA_ASYMMETRY` | Spherical aberration, a brightness profile `1 + e (3 rho^4 - 1)` (mean 1): `e > 0` a bright rim (soap bubble), `e < 0` centre-weighted (soft). `e = SPHERICAL * mix(1, sign, SA_ASYMMETRY)` with `sign` +1 behind the focus and -1 in front, so an asymmetry of 1 shows a rim on one side of the focus and a soft disc on the other |
| `ONION_AMPLITUDE` `ONION_RINGS` | Concentric ripple `1 + a cos(2 pi n rho)` |
| `APODIZATION` | Gaussian radial transmission of this standard deviation in pupil radii (smooth bokeh) |
| `OBSTRUCTION` | Central obstruction radius ratio: mirror-lens donut bokeh |

**Energy.** The shape is normalised by its area as a fraction of the circle
(integrated per gather pixel with 96 samples) and the profile by its mean over
the shape on each side of the focus. A pupil change reshapes and redistributes
the light of a defocused source but does not change the frame's exposure: the
energy test holds at the phase-1 tolerance for every variant.

The rim coordinate for the profile is the shape's own boundary, so a polygon
has a bright rim along its edges. The profile also modulates the near layer's
coverage, so the aberration shows on foreground bokeh too.

The shape is evaluated against the field position of the gather pixel (the
destination), not of each source, and the region test is also at the
destination; the difference is a disc radius, small against the field.

### Longitudinal chromatic aberration

A `LONGITUDINAL_CA` element shifts the focus of each colour channel in
dioptres (`1/s\x27 = 1/s + D`), `params[0..3]` for red, green, blue, weighted by
the region coverage and summed where several overlap. Green is the reference
the focus field was resolved for, so only the differences from green matter.
Each channel gathers with its own circle of confusion,
`c_ch = c + A f dD / (s1 - f)` (the first-order change, with `s1` the lens's
focus distance), clamped to `max_coc_px`.

With red and blue shifted toward farther focus than green (negative `D`), a
foreground edge has larger red and blue circles, so it fringes purple at the
edge of its bokeh; a background edge has the larger green circle and fringes
green. The size of the effect is `A f / sensor_width * width` pixels per
dioptre. A pixel with no shift runs one gather as before; a shifted one runs
three (one per channel), so the cost triples inside the region. Both quality
tiers do the same; the economical tier only has fewer taps.

### Ray map

The rendered image is rectilinear at the lens's focal length. The ray map says
where on it each sensor position looks, per wavelength, and is applied in
`cs_composite` **after** the defocus, which runs in the undistorted scene.
Elements apply in stack order to a point in focal-length units (`x = p_mm /
f`, y down: the tangent coordinates of the rendered image, as in OpenCV's
normalised camera coordinates). Each element's region is tested at the *sensor*
position and blends the element's map with the identity across its feather.
Order matters: `[SHIFT, BROWN_CONRADY]` distorts about a shifted axis,
`[BROWN_CONRADY, SHIFT]` shifts the distorted image.

| Element | `params` (see `components.rs`) | Map |
| --- | --- | --- |
| `PROJECTION` | model; `k1..k4` (Kannala-Brandt) or `d`, `s` (Panini) | rectilinear (identity), equidistant `r = θ`, equisolid `2 sin(θ/2)`, orthographic `sin θ`, stereographic `2 tan(θ/2)`, Kannala-Brandt `θ + k1 θ³ + …`, Panini, cylindrical |
| `BROWN_CONRADY` | `k1..k6`, `p1`, `p2` | OpenCV rational radial + tangential model, `x_d = x · (1 + k1 r² + k2 r⁴ + k3 r⁶)/(1 + k4 r² + k5 r⁴ + k6 r⁶) +` tangential. `k1 < 0` is barrel |
| `DIVISION` | `l1`, `l2` | `x = x_d / (1 + l1 r_d² + l2 r_d⁴)` |
| `ANAMORPHIC_DISTORTION` | `kx1`, `kx2`, `ky1`, `ky2` | per-axis radial polynomials (mumps) |
| `ANAMORPHIC_SQUEEZE` | factor | the horizontal field of view is that many times wider |
| `SHIFT` | `x`, `y` (image heights) | optical centre offset (tilt-shift) |
| `KEYSTONE` | `kx`, `ky` | `x / (1 + kx x_x + ky x_y)` |
| `FOCUS_BREATHING` | per dioptre, reference dioptres | magnification `1 + k (D − D_ref)` for the lens's focus `D` (after autofocus) |
| `PRISM` | count 2..8, offset, angle, ring, spin, falloff | weighted displaced / rotated copies (below) |
| `LATERAL_CA` | `d1`, `d2` | magnification `1 + d1 t + d2 t²`, `t = (λ − 550 nm)/150 nm` |
| `FLAT_PORT` | index, dispersion | a ray in water at `θ_w` reaches the camera at `sin θ = n sin θ_w`: magnification ≈ n, pincushion, dispersion |
| `DOME_PORT` | radius, pupil offset, index, dispersion, curvature | refraction at the dome for a pupil off its centre, plus field curvature from the virtual image |

Brown-Conrady and the anamorphic distortion are specified as the *forward*
model (ideal point to where the sensor records it), as calibrations are; the
sensor-to-scene map needs the inverse, which the shader finds with eight
Newton steps. A distortion with no inverse over the frame (a barrel stronger
than its polynomial can invert at the edge) falls back to whatever the
iteration reaches; keep the polynomial monotonic over the frame.

**Defocus size.** A circle of confusion is a size on the sensor, but it is
gathered in the undistorted scene, where the map stretches it by the local
magnification. `cs_coc` multiplies the circle by `sqrt(det J)`, `J` the
Jacobian of the sensor-to-scene map at the sensor point that looks at the
pixel (found with Newton steps), so blur comes out the size optics says. The
correction is isotropic: a map that stretches one axis more than the other
blurs by the mean.

**Prism.** Copy `i` of `N` samples the mapped image displaced by an offset (a
line centred on the original, or a ring) after turning it `i · spin` about the
frame centre, weighted `falloff^i` and normalised to sum to 1, so the filter
redistributes light and never changes the frame's energy. The first prism
covering a pixel acts; across its region's feather it blends with the unprismed
image.

**Underwater ports.** The flat port's refraction (n = 1.333, about 0.006
dispersion for water) is exact for a pupil on the port; the dome port's follows
the law of sines in the triangle of pupil, dome centre and glass, `θ_w = θ − i +
asin(sin i / n)`, `sin i = (e/R) sin θ`, which is no deviation for a centred
pupil. The dome's virtual image of a distant subject is a sphere of radius
`R n / (n − 1)` about its centre, so the focus field gains `(1/cos θ − 1) / ρ`
dioptres toward the corners (`CURVATURE` scales it).

**Spectral lateral chromatic aberration.** A `LATERAL_CA` element, or a port
with dispersion, gives each wavelength its own map. The composite samples N
wavelengths over 400–700 nm (3, 7, 16 for `quality` 0, 1, 2), each channel
takes a Gaussian sensor response of them, and the weights are normalised so
each channel's sum is exactly 1: a lens whose wavelengths agree reproduces the
image. A wavelength's map is the stack evaluated at that sample's `t` (−1 blue
… +1 red), so `d1 > 0` magnifies red more than blue and a white line off the
axis splits red outward and blue inward. The cost is N full defocus fetches
per pixel.

**Edges.** Outside the rendered frame the map clamps to the edge pixel (it does
not fade): where a distortion or fisheye looks past the frame it repeats the
last row or column.

### Overscan

The map's reach beyond the frame is published as `ResolvedLens::
required_overscan` (and by `ray_map::required_overscan` on the CPU, from the
lens row and its elements): the factor, at least 1 and exactly 1 for a map that
stays inside the frame, by which the scene extent at the frame's boundary
exceeds the frame's half extent, over the spectrum's ends (`t = -1, 0, 1`) and
every prism copy. It is sampled at 64 boundary points (16 per edge, corners
included) and capped at 16; a map that folds back inside the frame can reach
further than its boundary does.

**What a camera does with it.** The camera widens its projection's *tangent
extent* by the lens's applied overscan `R` about the frame centre
(`helio::Camera::with_overscan(R)`, which scales the `x` and `y` rows of the
projection by `1 / R`) and renders that. The pass is told `R` in the lens row
(`LensComponent::overscan`, 0 reads as 1; the resolved value is
`ResolvedLens::overscan`) and treats its input as the frame widened by `R`:

- the composite looks the ray map's scene position `s` up at the image position
  `s / (aspect, 1) / R + 0.5`, so the final framing is unchanged, the centre
  stays on its pixel, and the corners read real pixels where they used to clamp;
- everything the pass evaluates *in the scene* uses the frame's coordinates, not
  the image's: element regions, the focus field and its field radius, the pupil
  (cat's eye, baked footprints), the map's magnification for the circle of
  confusion, a USER hook's `uv`, the autofocus point (a position in the frame)
  and the circle of confusion's pixel size (the sensor is the frame's width, which
  is `1 / R` of the image's);
- the aberration stage and the transmission act on the sensor image, which is the
  frame, and are unchanged.

An overscanned image always takes the mapped path (`MAP_REMAP`), so `R > 1` with no
ray-map element is a plain crop.

**How much.** `ray_map::applied_overscan(lens, elements, aspect, rays)` is the
required overscan, snapped to 1 below 1.001 and capped at
`APPLIED_OVERSCAN_CAP` (2). It is the one number the camera widens by and the
lens row carries, so compute it once from the authored lens (the editor-side
`camera_lens_view_overscan` does, see "Overscan" in `.agents/LENS.md` at the repository root). It
needs the lens's aspect: the lens's field of view follows the sensor width across
the image, so a wider image has a narrower view and reaches less far. `rays` are
the CPU ray maps of the bank layers the stack names (`PrescriptionBank::rays`,
`bundled_rays`; empty: `PRESCRIPTION` elements are inert), so the CPU value
equals the GPU's for everything but `USER` elements and mask regions, which only
the GPU runs (a user ray map's reach is not part of the CPU value; if one reaches
past the frame its host has to widen the camera itself, or accept the clamp).

**Resolution against cost.** The camera renders the same pixel count over the
wider view, so the final frame has `1 / R` of the pixels per unit of view angle
at its centre (R = 1.2 loses 17%, 2 halves it). The alternative, scaling the
render target by `R`, keeps the centre sharp but costs `R^2` in every pass and
resizes every pass's targets whenever an authored lens changes, a hitch on each
inspector edit. The cap of 2 bounds the loss at a half; a map that reaches further
(an 8 mm fisheye reaches 16) still clamps at the widened image's edge, as it did
at the frame's. Only lenses that look past the frame pay: a lens that stays
inside has `R = 1` and the camera and image are untouched, bit for bit.

Post-lens passes that draw in screen space (editor gizmos, debug lines, the
flare pass's analytic sources and its depth test) still use the widened camera,
so they sit where an unwidened camera would not put them; lens flare sources
under overscan are approximate.

### Transmission

The product of the elements covering a sensor position, per colour channel,
applied to the composite:

| Element | Effect |
| --- | --- |
| `NATURAL_VIGNETTE` | `cos⁴ θ` of the ray's real angle after the map (so a fisheye follows its projection), mixed by the amount. The only natural falloff: the flare and post-process passes have none of their own |
| `OPTICAL_VIGNETTE` | iris share left by the barrel: the unit circle against a circle of `BARREL_RADIUS` displaced `CATS_EYE · r` toward the axis, the geometry of the `PUPIL` cat's eye. With a reference f-number the barrel is a fixed size, so it clips less as the iris shrinks (`barrel · N / N_ref`) |
| `MECHANICAL_VIGNETTE` | circular hood or rotated rectangular matte box with a smooth feather |
| `ND_FILTER` | `2^−stops` |
| `GRADUATED_ND` | colour-tinted density ramping across `softness` about a line (hard ≈ 0.01, soft ≈ 0.5) |
| `COLOR_FILTER` | linear transmittance, mixed by the amount |
| `T_STOP_LOSS` | `(f_number / t_stop)²`, mixed by the amount |

`t_stop` is carried on the lens and applied only when a `T_STOP_LOSS` element
asks for it, so existing lenses keep their exposure.

### In-focus aberrations, filters and contaminants

Elements 24..37 act on the **sensor image** (after the defocus, the ray map and
the transmission), in the `cs_ab_down` / `cs_ab_apply` stages. They are placed
last because their point spreads live in sensor coordinates (the field
position of a pixel is its position on the sensor) and because a point spread
convolves a defocused image exactly as it does a sharp one. The one deviation
from optical order is that a front filter would scatter light *before* the
lens's vignetting; here the scatter spreads light already vignetted, a
difference confined to the smooth field falloff.

Without such an element nothing changes: the composite writes `lens_color`
directly and the two stages dispatch zero groups (`aberration_flags`,
`ResolvedLens`). With one, the composite writes an intermediate (`composed`),
`cs_ab_down` builds a pyramid of it (like the flare pass's: level 0 is half
the image, each next a stride-2 4x4 box of the one before, so every level has
the image's mean radiance), and `cs_ab_apply` writes `lens_color`.

A **scatterer** moves a share `f` of every source's light over a kernel that
sums to 1. As a gather, the pixel loses `f` of its own light and collects
`f * sum(kernel * source)`, so the frame's mean radiance is unchanged (to the
frame edge, which clamps). The shares of all elements add and are scaled
together if they would exceed one. A Gaussian of standard deviation `s` pixels
is read from the pyramid level whose texels are about `1.6 s` wide, from four
taps. Kernels are in image-height units, so they scale with the frame.

| Kind (`element_kind`) | `params` (slot: meaning) | Effect |
| --- | --- | --- |
| `SPHERICAL_GLOW` 24 | 0 `AMOUNT`, 1 `REFERENCE_F_NUMBER` (2.8), 2 `EXPONENT` k (2), 3 `RADIUS` (0.012), 4 `TAIL` (0.55) | Soft-focus halo; share `AMOUNT (N_ref / N)^k` |
| `COMA` 25 | 0 `AMOUNT` share at the corner, 1 `LENGTH` at the corner, 2 `EXPONENT` of the field radius in the share (1), 3 `SHAPE` taper (2) | Comet tail along the radius, away from the axis; share `AMOUNT r^e`, length `LENGTH r` |
| `ASTIGMATISM` 26 | 0 `AMOUNT` (signed sigma at the corner, image heights), 1 `EXPONENT` (2), 2 `MINOR_RATIO` | Elliptical Gaussian PSF aligned with the radius: `+` long axis radial, `-` tangential; long sigma `abs(AMOUNT) r^e` |
| `CORNER_SOFTNESS` 27 | 0 `SIGMA` at the corner, 1 `EXPONENT` (2), 2 `START` radius | Isotropic blur `SIGMA ((r - START) / (1 - START))^e` (MTF falloff) |
| `DIFFUSION` 28 | 0 `STRENGTH`, 1 `RADIUS` (0.01), 2 `TAIL` (0.5) | Four Gaussians, sigma `RADIUS 2^j`, weights `TAIL^j` |
| `PRO_MIST` 29 | as `DIFFUSION`, radius 0.012, tail 0.65 | Wide halo; lifts the far shadows |
| `BLACK_PRO_MIST` 30 | as `DIFFUSION`, radius 0.012, tail 0.3 | Same share, halo stays near the highlight |
| `GLIMMERGLASS` 31 | as `DIFFUSION`, radius 0.004, tail 0.35; 3 `GLINT` contrast (0.8) | Small halo times `1 + GLINT (2h - 1)`, `h` a per-pixel hash (mean 1) |
| `STAR_FILTER` 32 | 0 `POINTS` 2..16 (4), 1 `ROTATION`, 2 `LENGTH` (0.2), 3 `INTENSITY`, 4 `FALLOFF` (1) | `POINTS` spikes at `ROTATION + 2 pi k / POINTS`, density `d^-FALLOFF`, 20 taps each from 2 px to the length |
| `HALATION` 33 | 0 `STRENGTH`, 1 `RADIUS` (0.02), 2..4 `TINT` rgb (1, 0.35, 0.08), 5 `TAIL` (0.5) | Channel `c` scatters `STRENGTH TINT[c]` of its light into two lobes (`RADIUS`, `2 RADIUS`); each channel conserves its own energy, so the halo is red-orange and the core loses a little green and blue |
| `WATER_DROPS` 34 | 0 `REFRACTION` (image heights at a unit normal), 1 `NORMAL_LAYER`, 2 `DEFOCUS` (sigma, image heights) | In the region (the footprint) the image is read at `-REFRACTION n`, `n` from the red and green of the mask layer, and defocused |
| `SMEAR` 35 | 0 `STRENGTH`, 1 `LENGTH` (0.03), 2 `ANGLE` | Gaussian line (sigma `LENGTH`) along `ANGLE` |
| `FOG` 36 | as `DIFFUSION`, radius 0.03, tail 0.8 | Use a mask region for patchy breath fog |
| `DIRT` 37 | 0 `STRENGTH`, 1 `RADIUS` (0.02), 2 `LOSS`, 3 `TAIL` (0.5) | Diffusion where covered, and `LOSS` of the light absorbed there |

A slot given as `(x)` takes that default when it is 0 or less. The region
weight multiplies the share (or the sigma, for the Gaussian PSFs): it is read at
the destination pixel, so a region that cuts through a bright source scatters
only as much as lands inside it, and energy is conserved for regions that do
not (full frame, or wherever the source's neighbourhood lies wholly inside or
outside).

Coma's tail length varies with the destination pixel; the gather normalises per
destination, not per source, so its energy is conserved to the slow variation of
the length (a fraction of a per cent in the test). Astigmatism and corner
softness are blurs that replace the sharp pixel (the scatterers then act on
that base); they conserve energy to the same degree.

**Masks.** A `MASK` region reads channel `CHANNEL` of layer `LAYER` of the
optional `lens_masks` array (`MASKS`; RGBA8, built with `mask_array`) as its
coverage, bilinear, at `(uv - 0.5) / SCALE + 0.5 - OFFSET`, optionally tiling
(`mask` slots in `components.rs`); the feather does not apply. Without an array
bound a 1x1 transparent layer is, so a mask region covers nothing. The array is
read with `textureLoad`, so every stage that evaluates a region binds it
(`cs_resolve`, `cs_coc`, `cs_gather`, `cs_composite`, `cs_ab_apply`).
`water_drop_mask(size, drops, radius, seed)` generates a deterministic layer of
spherical-cap drops (red/green normal `n * 0.5 + 0.5`, blue height, alpha
footprint); use alpha as the footprint of a `WATER_DROPS` element on the same
layer.

### Seams

A diopter sits in front of the lens, out of focus, so its edge is soft: the
entrance pupil seen from the filter, `A / e` radians for a filter `e` metres
ahead of the lens (`params[2]`, default 0.08 m). The default feather is derived
from that and the aperture, so a seam narrows as the lens stops down; an
authored `feather` (image-height units) replaces it. `params[1]` darkens the
seam.

Across a seam a pixel sees both focus configurations. The two strongest are kept
per pixel (`coc_a`, `coc_b`, weight, at most one half) and the gather runs once
for each, blended by the weight.

### Baked prescriptions

`PRESCRIPTION` (38) puts a real lens, traced and baked on the CPU by
`helio-lens-optics`, in the stack. It reduces the same per-pixel quantities as the
parametric elements, from tables instead of formulas:

| Quantity | From the bake | Replaces (weight `1 - w` where the prescription covers with weight `w`) |
| --- | --- | --- |
| Ray map | `RayMapLut.tan_theta`, per wavelength and field: the sensor radius `r` (mm over the baked half diagonal) looks along `tan theta(r, lambda)` in its own direction. Linear in the field (continued past the baked corner), linear between the bake's wavelengths, clamped beyond them. Distortion and lateral colour are both in it | `PROJECTION`, `BROWN_CONRADY`, `DIVISION`, `ANAMORPHIC_DISTORTION`, `LATERAL_CA` |
| Transmission | `VignettingLut`: `cos^4` of the traced image-space chief ray times the share of the stop the clear apertures leave, linear in the field and in `log2(f-number)`, clamped to the baked range | `NATURAL_VIGNETTE`, `OPTICAL_VIGNETTE` |
| Pupil | `PupilFootprints`: the stop's coverage at the field (two baked fields blended linearly), at the baked f-number. Stopped down to `N` it is sampled over the central `N0 / N` of the stop, which is exact (a ray is aimed at an absolute stop position). It multiplies the iris shape, so blades, squeeze and swirl still apply | the `CATS_EYE` and `BARREL_RADIUS` of a `PUPIL` element (ignored where `w >= 0.5`) |
| Focus field | `FocusData.best_mm`: field curvature as extra dioptres (`-dz / f^2`, `f` the effective focal length, zero on the axis) | the lens row's `field_curvature` |
| Longitudinal CA | `FocusData.axial_real_mm` weighted with the sensor response of each channel, as dioptres in the same units; the shader subtracts green, so only the differences matter | `LONGITUDINAL_CA` elements |
| In-focus point spread | `PsfTable` at each field's own best focus (so its field curvature is not counted twice): a Gaussian ellipse of the radial and sagittal RMS (added in variance to the aberration stage's), and a coma tail from the centroid shift and the edge radius, with the tail's own variance taken out of the core | `ASTIGMATISM`, `CORNER_SOFTNESS`, `COMA` |

Nothing else is replaced: mechanical vignetting, filters, `T_STOP_LOSS`, ports,
diopters, prisms, `SPHERICAL_GLOW` and the diffusion elements stack on top of the
bake. The **first** `PRESCRIPTION` whose region covers a pixel is the one that
acts there; `1 - w` blends across its feather, so inside it nothing parametric is
applied twice and outside it nothing changes.

Parameters: `params[0]` is the layer of the `PrescriptionBank`; `params[1]` scales
the point spread (1 the bake, 0 none; a zeroed row applies none).

**Focal length.** A prescription whose region is the **full frame** is the view's
lens: `cs_resolve` sets `focal_length_mm` to the bake's effective focal length and
keeps `f_number` at or above the widest aperture the vignetting table was baked at
(and `t_stop` at or above that), and sets `natural_vignette` to 1 for the flare
pass (the bake's `cos^4` applies to its analytic sources too). The camera has to
render the field of view of that focal length on the sensor (read it from
`PrescriptionBank::efl_mm` or `lens_resolved`). A partial-frame prescription keeps
the row's optics: the ray map is still the bake's tangent, so it is only
consistent where the row's focal length matches.

**Host API.** `PrescriptionBank::new(&device, &[&LensBake, ..])` uploads the bakes
as one read-only storage buffer of 32-bit words (a directory, a header per lens,
then the tables; not textures: every lens keeps its own table sizes). Layer `i` is
`bakes[i]`. Register `bank.buffer()` under `PRESCRIPTIONS` (`"lens_prescriptions"`)
in the graph's registry like `lens_masks`, or pass it as
`LensInputs::prescriptions`; with none a bank of no lenses is bound and the
element covers nothing. `bank.efl_mm(layer)` and `bank.min_f_number(layer)` give
the numbers the full-frame rule applies.

**Static and dynamic.** The bank is immutable (build a new one to change a lens).
Baked in: the prescription, the focus distance the bake was focused at, the
spectral samples, the f-number *table* of the vignetting (from the bake's
`f_numbers`), the footprint f-number, the field grids. Dynamic per frame, no
rebake: the f-number (vignetting interpolates its table; footprints scale), the
focus distance and autofocus (the focus field adds the baked curvature and
chromatic shifts to whatever focus is resolved; the bake's own focus is only that
of its sensor position), the sensor size and quality (wavelength samples), and
which pixels the region covers. The chromatic path (`MAP_SPECTRAL`) switches on
only for a bake whose wavelengths disagree: an ideal lens costs no spectral
samples.

### User elements

`USER` (39) runs WGSL the host compiles into the lens shader:
`LensPass::new_with_user(device, w, h, Some(snippet))`, or the
`LensUser(snippet)` renderer setting, which the presets' lens node builds with
(the lens counterpart of the post-process `UserEffects`). The snippet replaces
the stock no-op block of `lens.wgsl` (between the `%%USER_BEGIN` and `%%USER_END`
markers) and may define any of:

```wgsl
fn user_ray_map(uv: vec2<f32>, params: array<f32, 16>, t: f32) -> vec2<f32>
fn user_transmission(uv: vec2<f32>, params: array<f32, 16>) -> vec3<f32>
fn user_focus(uv: vec2<f32>, params: array<f32, 16>, s: f32) -> f32
```

`uv` is the point in the unit square (y down); `params` the element's 16 floats;
`t` the wavelength position (-1 blue .. 1 red); `s` the focus in dioptres so far,
and `user_focus` returns the new one. A hook the snippet omits is the identity.
The row's `flags` select the hooks that run (`user::MAP`, `TRANSMISSION`, `FOCUS`,
and `SPECTRAL` when the map depends on `t`), the element's region blends each with
its identity by the coverage, and the ray map hook acts in stack order on the point
the stack has reached. Without a snippet the shader is the stock one and `USER`
elements are not even scanned: no stage flag, no cost, bit-exact output.

### Regions

Regions live in image-height units centred on the frame, `q = (uv - 0.5) *
(aspect, 1)`, y down: full frame, half-plane `{angle, offset}`, ellipse
`{centre, radii, rotation}`, convex polygon of up to 8 vertices (either
winding), radial band `{r0, r1, centre}`. A mask-texture channel
(see Masks above) covers nothing until a mask array is bound. `LensRegion`
builds the rows.

### Bundled resources

A host that authors no lens data of its own calls `LensPass::with_bundled_resources()`
(the default graphs do) and gets, as the registry entries above:

| Entry | Contents |
| --- | --- |
| `lens_prescriptions` | the three example lenses of `helio-lens-optics`, baked with the default `BakeConfig`, layers in `BundledLens` order: `ThinLens50F2` (0), `CookeTriplet50F5` (1), `DoubleGauss50F2` (2). Append new lenses, never reorder: levels store the layer |
| `lens_masks` | one layer (`WATER_DROP_MASK_LAYER`, 0): 70 water drops of 0.8-3.5% of the width on 512 x 512, normals in red/green, footprint in alpha (`water_drop_mask`, seed 7), what the "Rainy lens" preset reads |
| `lens_aperture_shapes` | `BundledAperture` layers on 128 x 128: `Star` (0), `Heart` (1), `Triangle` (2), for `pupil` `TEXTURE_LAYER` |

The lenses are traced once per process on a background thread
(`start_baking`, shared by every pass and by `bundled_rays`, the CPU ray maps the
overscan needs) and the bank is uploaded when they are done, so renderer start is
not held: until then a `PRESCRIPTION` element covers nothing. Bake time on the
reference machine: about 0.8 s optimized, 2.1 s unoptimized (the root workspace
builds `helio-lens-optics` optimized in dev). The masks and shapes are made on
the first frame. The entries are published before the passes run
(`publish_frame_inputs`), so the first frames after a bake finishes already see
the bank. A host with its own data skips the call and registers its own under the
same keys.

## Settings

Two packed SceneDB components, read straight from GPU memory:

| Row | Buffer | Matched by |
| --- | --- | --- |
| `LensComponent` | `camera_lenses` | `view_id` (u32 bits of the camera's `jitter_frame.w`); the first enabled row wins, none disables the lens |
| `LensElementComponent` | `lens_elements` | rows `element_first .. element_first + element_count` of the owning lens (`element_count` is capped at 32; a `NONE` row is skipped) |

The lens is owned by the camera. Post-process volumes do not blend it.

| `LensComponent` field | Meaning |
| --- | --- |
| `enabled`, `view_id` | Row switch and view match |
| `focal_length_mm`, `f_number`, `t_stop` | Optics; `t_stop` loses light against the f-number through a `T_STOP_LOSS` element |
| `sensor_width_mm`, `sensor_height_mm` | Sensor; height 0 derives 3:2 |
| `focus_mode`, `focus_distance_m` | 0 manual; 1 autofocus on `af_u`, `af_v` |
| `af_radius`, `af_speed` | Autofocus sample radius in uv (0: 0.02) and ease rate in 1/s (0: instant) |
| `quality`, `max_coc_px` | 0 economical (24 taps, 3 wavelengths), 1 high (56, 7) or 2 ultra (56, 16); circle cap in pixels (0: 32) |
| `element_first`, `element_count` | The element rows |
| `tilt_angle`, `tilt_axis`, `field_curvature` | Focus field |
| `aperture_blades`, `aperture_rotation` | Iris: the base pupil polygon of the gather, and the flare's |
| `overscan` | The factor the camera widened the field of view by (0 or 1: none, at most 2); see Overscan |

Autofocus averages a grid of depth taps in dioptres, eases in dioptres at
`af_speed`, and holds its last focus when the point sees only sky.

## Stages

All dispatches are sized on the GPU by `cs_resolve`, so a view with no lens costs
one workgroup and a copy, with no CPU readback.

1. `cs_resolve`: matches the row, runs autofocus, scans the stack for the
   stages it needs (`map_flags`), measures the ray map's `required_overscan` on
   64 boundary samples, writes the `lens_resolved` buffer and the indirect
   arguments (zero when disabled, except the copy).
2. `cs_coc`: per-pixel circle of confusion from the focus field, scaled by the
   ray map's local magnification; depth is read with a nearest-of-footprint
   fetch when it is rendered below the image's size.
3. `cs_tiles`, `cs_dilate`: 16x16 tile extents of the circles, widened by every
   circle that reaches the tile, bounding each pixel's gather radius and
   skipping the near field where there is none.
4. `cs_prefilter`: half-resolution colour and circles; a block with a near-field
   pixel takes the near class.
5. `cs_gather`: scatter-as-gather. Each source spreads its radiance over a disc of
   its own circle, shaped by the pupil function and weighted by one over the
   disc's area. Far-field samples are
   averaged (a mean, so a uniform blur keeps the image's energy); near-field
   samples form a layer whose alpha is the fraction of the pixel covered, over
   the far layer. Taps are a Vogel spiral rotated per pixel.
6. `cs_composite`: in-focus pixels keep their full-resolution value, blurred ones
   take the far layer, the near layer goes over both; with a ray map this runs
   at the position the map says, once per wavelength sample and prism copy, and
   the spectral weights and the transmission are applied. A lens with no map
   elements copies texel for texel; a disabled lens copies the input. With an
   aberration element the result goes to the `composed` intermediate instead
   of `lens_color`.
7. `cs_ab_down` (one dispatch per pyramid level, at most 8, level 0 half the
   image) and `cs_ab_apply`: only for a lens with an aberration, filter or
   contaminant element (`aberration_flags`); both are zero-sized otherwise. See
   "In-focus aberrations, filters and contaminants". They follow the composite
   so the point spreads act in sensor coordinates on the defocused, mapped,
   vignetted image.

## Graph contract

```text
FogCompositePass -> Transparent -> TSR/FXAA -> LensPass -> LensFlarePass -> PostProcessPass
                                   (tsr_color | fxaa_color | fogged_hdr)
```

- Reads the configured HDR input (`with_color_input`; default `fogged_hdr`,
  falling back to `pre_aa`) and the scene depth. Depth may be at internal
  resolution while colour is at output resolution after TSR upscaling.
- Writes `lens_color`: Rgba16Float, scene-linear, the size of its input.
  `LensFlarePass` and `PostProcessPass` read it.
- Publishes `lens_resolved`: a storage buffer holding one `ResolvedLens` (88
  bytes: the optics, `natural_vignette`, `required_overscan`, `map_flags`,
  `spectral_samples`, then `aberration_flags` and `overscan`, appended behind the
  fields other passes read). It
  holds the neutral 50 mm f/2.8, 36 mm, six-blade lens when the view has none, so
  consumers always have optics. `LensFlarePass` takes f-number, focal length,
  sensor width and the iris, and the natural falloff for analytic sources,
  from it; post-process settings no longer carry them (nor chromatic
  aberration or a lens vignette: both are the lens's).
- Records on the graphics encoder after its producers. The compute encoder is
  submitted before graphics and would sample the previous frame.
- Hosts register the columns up front, like the other SceneDB rows:
  `LensComponent::register_gpu_columns_growable` and
  `LensElementComponent::register_gpu_columns_growable`. A graph without the
  `camera_lenses` buffer runs with no lens.
- Reads the optional registry entry `lens_aperture_shapes`
  (`APERTURE_SHAPES`): a `D2Array` view of R8Unorm iris shapes, see
  `aperture_shape_array`. Without it one white 1x1 layer is bound, so the
  binding is always valid.
- Reads the optional registry entry `lens_masks` (`MASKS`): a `D2Array` view of
  RGBA8 layers, see `mask_array`. Without it a transparent 1x1 layer is bound.
- Reads the optional registry entry `lens_prescriptions` (`PRESCRIPTIONS`): the
  storage buffer of a `PrescriptionBank`. Without it a bank of no lenses is bound.
- `ResolvedLens::map_flags` gains bit 8 (`MAP_PRESCRIPTION`).
- `LensPass::with_bundled_resources()` registers all three optional entries from
  the host's side, see "Bundled resources".

## Limits

- The aberration stage runs after the ray map and the transmission, on the
  sensor image; a filter that physically sits in front of the lens scatters
  light the vignetting would already have dimmed. Its pyramid is built from
  the whole frame, so the frame's edge clamps: a source near the edge loses the
  part of its kernel that leaves the frame, and the edge texels are repeated
  into the kernels of pixels inside it (negligible against a dark border).
- A region on a scatterer weights the destination pixel, so energy is conserved
  only for regions that do not cut through the sources (full frame, or a source
  wholly in or out). Overlapping scatterers add their shares; a total above 98%
  is scaled together.
- Gaussian kernels are taken from a box pyramid with four bilinear taps, which
  is a few per cent off a true Gaussian and shows faint blockiness for the
  widest lobes. A star's spikes widen with distance (their taps read coarser
  levels), a cone of a few degrees; coma and astigmatism are gathers with a
  kernel set at the destination, conserving energy only to its slow variation.
- Water drops refract the sensor image with no depth: the lookup is a pure
  displacement, drops do not occlude or reflect, and the defocus inside them is
  a Gaussian, not a thin-lens circle.

- Screen-space: nothing outside the frame or behind opaque geometry contributes,
  so occluded background is reconstructed from its surroundings.
- The ray map resamples the defocused image bilinearly at the pixel's scene
  position, so a strong minification or a heavy distortion aliases a fine
  pattern the blur does not hide. The defocus size correction is isotropic, and
  the focus field, the pupil and the seams are evaluated in the undistorted
  scene.
- The map depends on the wavelength only for `LATERAL_CA` and dispersive ports;
  every other element is one map for all colours. Transmission is evaluated
  from the green wavelength's ray. Analytic flare sources take only the natural
  falloff.
- Outside the rendered image the map clamps to the edge pixel. A camera that
  renders the overscan (see Overscan) leaves that to maps that reach further than
  the cap of 2.
- The pupil is evaluated at the gather pixel, not per source, and a `PUPIL`
  region switches at 50% coverage with no blend. Pupil shape detail is limited
  by the half-resolution gather: a disc under about eight pixels across reads
  as a blob whatever its blades.
- The pupil lies inside the circle of confusion, so a swirl or squeeze makes the
  disc smaller along its short axis rather than larger along its long one;
  `max_coc_px` bounds the long axis. A texture aperture is bounded by the
  circle as well.
- Longitudinal chromatic aberration shifts circles by the first-order change in
  the circle (exact for a lens focused far from its focal length) and widens the
  gather radius by it, but the tile reach (`cs_tiles`, `cs_dilate`) and the
  composite's in-focus test follow the green circle: a shifted channel's disc
  that extends beyond the tiles green reaches is cut at the tile edge, and a
  pixel in focus for green shows no chromatic blur. Inside the region the gather
  costs three times as much.
- The spherical-aberration, onion and apodization profiles redistribute light
  within the disc; sources at different circle sizes are mixed by area, so a
  scene mixing sharp and strongly blurred sources shifts slightly (below the
  3% of the energy test) where profiles differ.
- Half-resolution gather: blur of a few pixels is resolved at half size, and a
  half-resolution block that mixes near and far pixels takes the near class.
- Taps are noise-rotated per pixel, trading structure for grain at the cost of a
  fixed budget; very bright, very small highlights under a large circle are
  under-sampled.
- Seam handling keeps the strongest two focus configurations per pixel; a
  junction of three seams approximates.
- Elements address rows by index range; the authoring layer that places element
  rows is not part of this pass. The phase 4 kinds (24..37) are not yet exposed
  by the editor-side component.
- Prescriptions: rotationally symmetric lenses only (the bake's limit), so the
  azimuth only turns the radial direction. The footprint is used with the same
  orientation for sources in front of and behind the focus (a real cat's eye
  flips with the sign of the defocus). The point spread is the bake's geometric
  spot at its one f-number: the aperture dependence of spherical aberration
  (`SPHERICAL_GLOW`) is not derived from it and stays a parametric element.
  Opening wider than the baked f-number is clamped (full-frame) or extrapolated by
  clamping the table (partial). The bake's effective focal length is the
  small-field `y / tan` of an infinity-focused bake; a bake focused closer carries
  the magnified equivalent. The vignetting table's f-numbers must ascend. The CPU
  twin `ray_map::required_overscan` knows `PRESCRIPTION` only with the bank's rays
  (`required_overscan_with_rays`) and never `USER` (the GPU's `lens_resolved` does).
- User elements: the hooks run per pixel per wavelength sample (the ray map), so
  an expensive `user_ray_map` multiplies with the sample count.

## Validation

`cargo test -p helio-pass-lens` runs the production WGSL on a GPU (an adapter is
required; tests fail rather than skip). Phase 3 (`src/tests/optics.rs`) is checked
against closed forms where physics gives one: a rectilinear map is an exact
identity; a point under the OpenCV model lands where the formula says; a
fisheye records radius against angle for each projection (equidistant within
2%); spectral weights sum to 1 per channel and a neutral lens at 3, 7 and 16
samples reproduces the image within fp16; a white line off the axis splits red
outward and blue inward by the weighted prediction, and the axis line does not;
cos⁴ follows the ray's real angle (also through a fisheye); a graduated ND is
monotonic, hard against soft and tinted; the T-stop loss is `(N/T)²`; a prism
preserves energy for a line, a ring with falloff, a kaleidoscope and a region;
`required_overscan` is 1 for the identity and a map that stays inside, above 1
for barrel and fisheye, and the squeeze, shift, CA and prism extents exactly,
GPU against CPU; the flat port's magnification and dispersion and the dome's
refraction and focus curvature against their formulas; breathing against the
focus and the circle of confusion halved by a 2x zoom; the optical vignette
against the circle-circle area; every element against the CPU twin on a smooth
image (and the stack order mattering); and clamping outside the frame. The
older tests: exact passthrough with no or disabled
lens, row matching, the thin-lens circle against the formula, energy
preservation of the gather at both quality tiers, near-field coverage, the
split diopter (both halves in focus, the seam width against the aperture),
stacked three-way and nested diopters, every region kind, tilt and field
curvature, autofocus and its easing, depth below image resolution, and the
component ABI.

Phase 4 (`src/tests/aberration.rs`): a lens without such an element leaves the
image bit for bit and dispatches none of the new stages (and a zero-strength
element reproduces the image exactly); every scatterer (diffusion, Pro-Mist,
Black Pro-Mist, Glimmerglass, fog, glow, 4, 6 and 8 point stars, halation,
smear, dirt, coma, and a stack of four) keeps each channel's mean radiance
within 3% (measured under 1.5%); a star of 2, 4, 6 and 8 points makes that many
peaks in the angular energy histogram, at the rotation asked for; coma's centroid
moves outward along the radius at three field positions and not on the axis;
astigmatism's ellipse is radial for a positive amount and tangential for a
negative one; corner softness leaves the axis untouched; the glow follows
`(N_ref / N)^2`; smear spreads along its angle; Black Pro-Mist lifts far
shadows less than Pro-Mist at the same share; halation is red-dominant, tinted
as asked and per-channel conserving; dirt absorbs its loss; a `MASK` region
restricts an element to the white half (bit-exact elsewhere, inverted likewise,
nothing without a mask array); the water-drop generator is deterministic; a
drop displaces content opposite its normal inside its footprint, by the
expected amount, and leaves the rest of the frame bit-exact.

Phase 5 (`src/tests/prescription.rs`, the three example lenses of
`helio-lens-optics` baked in the tests): the bank's words (focal length, flags,
tables, zero field curvature on the axis); the ideal thin lens is the identity map
with no spectral path, no point spread, and its transmission is the bake's and the
parametric `cos^4` alike; the double Gauss puts a point where the bake's chief ray
says, per colour channel and weighted by the pass's spectral samples (0.08 px),
and its red-blue split equals the baked lateral colour; the GPU's
`required_overscan` equals the CPU reading of the bake's corner reach; a full-frame
prescription sets the focal length and the widest aperture (a partial one does
not); vignetting at f/2 is darker than at f/8 off axis and both match the table;
the bokeh is round on the axis and radially narrower than tangentially off it,
as the baked footprint is (control without it is round); a stack of the
parametric equivalents before and after a prescription is bit-identical to the
prescription alone, and with a half-plane prescription the left half is the
prescription alone and the right half carries `cos^4` once; the Cooke triplet is
held to its baked f-number and falloff; a made-up point spread on the ideal lens
blurs along the radius by the baked RMS (and across it by the other), conserves
light, leaves the axis alone, and a coma centroid shift moves the image outward
(positive) or inward (negative) by about the baked shift; the parametric
astigmatism, corner softness and coma give way to the baked spread bit for bit; a
bound bank nobody names, a missing layer, a missing bank and `USER` elements in a
lens built without a snippet leave the image, the stage flags and the dispatches
unchanged; baked field curvature defocuses the corners of the focal plane by the circle the
formula gives, and baked axial colour gives red, green and blue ordered circles
(and equal ones when the shifts are zero);
a `USER` ray map that flips `u` mirrors the image (also within a
region), and the transmission and focus hooks run while an omitted hook is the
identity.

The pupil tests measure the averaged result, not single taps (a lone point
source in a defocused plane is a few lucky taps, so its disc is noise): the
10-90% or 30-70% width of a blurred edge for the projected shape, and a small
bright blob binned by radius for the profile. They cover polygon blades (a
hexagon turned by a twelfth of a turn and a triangle against a circle,
curvature, wide-open rounding), anamorphic squeeze 1.33 / 1.8 / 2.0 within
10%, cat's eye (round on the axis, a narrower radial than tangential width off
it) and swirl, soap bubble / soft / near-far asymmetric spherical aberration,
apodization, onion rings, the donut hole, purple foreground and green
background fringes from longitudinal chromatic aberration, a texture aperture
and the neutral layer, region limiting, and the frame energy of fifteen pupil
variants at both quality tiers within 3%.

Overscan (`src/tests/overscan.rs`, `src/tests/bundled.rs`, and the CPU/GPU case
in `src/tests/prescription.rs`): an identity lens, a filter and a disabled lens
apply none; a 35 mm equidistant fisheye asks for more than 1.05 and an 8 mm one
is capped at 2 while its required overscan stays above it; the resolved lens
carries the row's overscan, sanitised (0, below 1, NaN: 1; above the cap: 2); on a
scene that is a linear function of the scene position, the overscanned fisheye
matches the CPU twin's scene position at every pixel including the corners
(within 4e-3), the same lens on an image that is only the frame reads clamped
corners (error above 3e-2), a bare overscan is a crop to the frame, a bright
feature at the frame centre keeps its pixel (within a tenth) with and without
the overscan, the circle of confusion is `1 / R` of the plain image's, and an
ellipse region sits in the frame, not the image; for the three bundled lenses and
stacks mixing a division model, a projection and partial prescriptions the CPU
`required_overscan_with_bank` equals the GPU's within 2e-3; the bundled lenses
bake in layer order with a 50 mm focal length each and rays equal to the bank's,
the iris shapes are distinct and inside the pupil, and the water-drop layer is
deterministic. `helio::Camera::with_overscan` is tested in `camera.rs`: factor 1
(and NaN) leave the projection bit for bit, a centre point stays on its pixel and
the old frame's edge moves in by `1 / R`.
