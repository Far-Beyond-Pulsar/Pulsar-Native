# Sky texture layer

The `AtmosphereComponent` (`plugins/vendor/sky`) draws the atmosphere, clouds,
procedural stars, shooting stars and the directional-light emitter disks (the
moon). The **sky texture** is one more layer in that stack: an
equirectangular panorama drawn *behind* all of them.

```
panorama (additive, × air transmittance × darkness)
  + atmosphere sky-view LUT           (already in the radiance)
  + directional-light disks / emitter disks (moon)
  + procedural stars + shooting stars (only when night_sky_enabled)
  → clouds composite over the result
```

Enabling the texture never disables a procedural layer and vice versa: they are
separate switches on the component (`sky_texture_enabled`, `night_sky_enabled`,
`shooting_stars_enabled`, `clouds_enabled`). The panorama is appearance only;
scene lighting (including moonlight) comes from `LightComponent`s.

## Controls (Sky Texture category)

| Property | Meaning |
|---|---|
| `sky_texture_enabled` | Off by default; saved levels load unchanged |
| `sky_texture_file` | A project texture asset (`TextureSrc`, picked with the asset picker, stored as its asset id; `.hdr/.exr/.png/.jpg`). Levels saved with a project-relative path still load. None chosen: procedural sky only |
| `sky_texture_intensity` | Radiance multiplier (default 10; the CC0 layers are physically faint) |
| `sky_texture_opacity` | 0..1 share added over the atmosphere |
| `sky_texture_{yaw,pitch,roll}_deg` | Orientation (yaw about Y, then pitch about X, then roll about Z) |

The panorama, stars and shooting stars fade by the directional lights'
brightness, not a horizon test: `sky_daylight` sums each light's luminance
weighted by its elevation, and `sky_darkness` maps that to 0..1 (full dark below
~0.05, full day above ~0.6; the default sun is ~2, the default moon 0.025). A
dim moon light being up does not end the night. The panorama is not a daytime
backdrop.

## Stars, constellations and shooting stars

All live in `atmosphere_composite.wgsl` (`night_radiance`) and are one
population: `point_light` (a core plus a faint halo, never drawn thinner than
about a pixel, its light spread to match so sub-pixel sources don't blink),
`star_color` (blue-white to orange), `star_radius` and `star_peak` (size and
brightness from a star's hash), all scaled by `star_brightness`.

**Constellation lines** (`constellation_lines`, Night Sky; off by default,
`constellation_radiance`): every procedural star (`star_grid`, `star_in_cell`,
the same stars `night_radiance` draws) is joined to every other star, each line
drawn until it would be too faint to see. A line's strength is the pair's
brightness product raised to `constellation_selectivity`, times a Gaussian of
their separation (`constellation_reach`, in star spacings, sqrt(4π/star_count)),
times `constellation_brightness` × star brightness (default 0.06: the web is
faint, never drawn on). Nothing is chosen at random: close bright pairs show,
far or faint ones fade out. A line is the stars' `point_light` across it at a
sub-pixel true width (it dims rather than thickens at low resolution), shaded
from one star's colour to the other's, and stops short of both stars. It adds
no stars. Each sky pixel gathers the stars within the longest visible line
(at most 5 cells up and down, 10 across, 28 stars) and tests each pair, so its
cost is bounded; a reach past that window is clipped to it.

**Shooting stars** (`meteor_radiance`): the head is one of the night's stars,
drawn exactly as the others, crossing a short great-circle path (5 to 25
degrees, 0.35 to 1.1 s at speed 1), starting 8 to 80 degrees up and heading
mostly downward, fading in and out at the path's ends. Each extra adds on top
and is off at 0:

| Property | Adds |
|---|---|
| `shooting_star_flare` | Light at the burn peak (a curve that peaks late and burns out fast), × the star's brightness |
| `shooting_star_trail` | A train behind the head, relative to it, falling to e^-2.5 at `shooting_star_length` |
| `shooting_star_afterglow` | Seconds (at speed 1) the train keeps glowing once the head is gone |
| `shooting_star_heat` | How white-hot the flare and train are (0: the star's own colour) |

Arrivals are a Poisson process at `shooting_star_rate` per minute: time slots of
fixed candidates, each present by chance, so several can share the sky at
irregular times and each pixel tests a bounded few.

These controls are words of `AtmosphereRow` (336 bytes) and its SceneDB twin
`AtmosphereSourceRow` (`meteors`, `constellations`), and of `AtmosphereParams`
in Helio's `atmosphere_common.wgsl` (`ATMOSPHERE_FRAME_BYTES` 2176).

## Data flow

- The row carries `sky_texture_source` (`helio_pass_sky::sky_texture::source`:
  none, or `FILE | index` into a table of resolved paths), `intensity`, `opacity`, `yaw/pitch/roll`.
  These reuse the six words that were reserved for a retired moon, so the row
  stays 76 words and old data is binary compatible. `AtmosphereRow`,
  `host_rows`, `atmosphere_common.wgsl` (helio submodule) name them identically.
- The CPU needs the source to keep the right texture resident, but the row is
  resolved on the GPU, so the component's SceneDB dispatch publishes it with
  `sky_texture::set_requested` (a level has one sky). The composite pass reads
  `sky_texture::requested()`.
- The component resolves `sky_texture_file` to a path (`sky_texture_path`):
  the asset id through the project file index (`path_for_asset_id_kind("texture", id)`,
  as every texture reference does), a legacy path under the project folder.
  An id the index does not know logs a warning and draws the procedural sky.
- `SkyTextureCache` decodes, caps (4096 wide) and mips on a worker thread, then
  uploads one `Rgba16Float` texture (bindings 11/12 of the composite). While it
  loads, a 1×1 black texture is bound; the layer is additive so nothing flashes.
  Turning the layer off frees the texture. Files reload when modified.
- Color space: `.hdr/.exr` are scene-linear; 8/16-bit formats are sRGB and are
  linearised on load.
- Mapping: image centre looks along -Z, top is +Y, u wraps around the horizon
  (`direction_to_uv` is the CPU twin of the WGSL, tested against it by text).

## CC0 presets (project assets, not in binaries)

No panorama is embedded and no build downloads anything: the sky feature has
no `build.rs`, so a binary linking the sky carries none of the ~25 MB the
presets used to add, and builds work offline.

The presets are **Deep-Space Skybox by Toolbench**, CC0 1.0
(https://toolbenchdev.com/gamedev/cc0-game-assets/deep-space-skybox/), galaxy
and nebula layers, starless. They are offered at editor time as **starter content** (`ui_level_editor::extend::StarterContent`,
registered by `sky_plugin`'s `editor.rs`; Tools menu, "Starter Content",
"Sky Panoramas (CC0 Deep-Space)"). The shell downloads the pinned archive on a
worker thread with progress and cancel and checks its SHA-256 against the
registration; `plugins/vendor/sky/editor/src/panoramas.rs` (`install`) then
writes into the project's `assets/sky/deep-space/`, through
`engine_fs::virtual_fs`: the two layers unchanged,
`deep_space_galaxy_nebula_4k.hdr` (galaxy plus nebula x6, the former default
look), and `CREDITS.md` / `LICENSE-CC0-1.0.txt`. The file index lists them as
texture assets; pick one in Sky Texture. A game carries only the file it uses.
The menu shows the licence, checksum and destination before install; the
outcome (done, failed with the reason, cancelled) is a notification and the
menu's last-attempt line. See "Starter content" in `PLUGIN_SYSTEM.md`.

## Known limits

- The installer needs the network and a local project. Not yet exercised in a
  real editor session: the menu, notifications and picker (the flow below them
  is covered headlessly by `plugins/vendor/sky/tests/starter_content.rs`).
- The sky pass keeps its own single resident panorama; it is not part of the
  SceneDB texture-asset/streaming system.
- The panorama does not rotate with the sun; orient it by hand.
- No image-based lighting or ambient contribution from the panorama.
