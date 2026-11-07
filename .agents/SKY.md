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
| `sky_texture_preset` | Galaxy + Nebula, Galaxy, Nebula, or Custom File |
| `sky_texture_file` | Custom File: `.hdr/.exr/.png/.jpg`, project-relative or absolute |
| `sky_texture_intensity` | Radiance multiplier (default 10; the CC0 layers are physically faint) |
| `sky_texture_opacity` | 0..1 share added over the atmosphere |
| `sky_texture_{yaw,pitch,roll}_deg` | Orientation (yaw about Y, then pitch about X, then roll about Z) |

The panorama, stars and shooting stars fade by the directional lights'
brightness, not a horizon test: `sky_daylight` sums each light's luminance
weighted by its elevation, and `sky_darkness` maps that to 0..1 (full dark below
~0.05, full day above ~0.6; the default sun is ~2, the default moon 0.025). A
dim moon light being up does not end the night. The panorama is not a daytime
backdrop.

## Data flow

- The row carries `sky_texture_source` (`helio_pass_sky::sky_texture::source`:
  none, preset, or `CUSTOM | index`), `intensity`, `opacity`, `yaw/pitch/roll`.
  These reuse the six words that were reserved for a retired moon, so the row
  stays 76 words and old data is binary compatible. `AtmosphereRow`,
  `host_rows`, `atmosphere_common.wgsl` (helio submodule) name them identically.
- The CPU needs the source to keep the right texture resident, but the row is
  resolved on the GPU, so the component's SceneDB dispatch publishes it with
  `sky_texture::set_requested` (a level has one sky). The composite pass reads
  `sky_texture::requested()`.
- `SkyTextureCache` decodes, caps (4096 wide) and mips on a worker thread, then
  uploads one `Rgba16Float` texture (bindings 11/12 of the composite). While it
  loads, a 1×1 black texture is bound; the layer is additive so nothing flashes.
  Turning the layer off frees the texture. Custom files reload when modified.
- Color space: `.hdr/.exr` are scene-linear; 8/16-bit formats are sRGB and are
  linearised on load.
- Mapping: image centre looks along -Z, top is +Y, u wraps around the horizon
  (`direction_to_uv` is the CPU twin of the WGSL, tested against it by text).

## Built-in presets and licensing

Galaxy and nebula layers of **Deep-Space Skybox by Toolbench**, CC0 1.0
(https://toolbenchdev.com/gamedev/cc0-game-assets/deep-space-skybox/). They are
separate from the star layer, so they are starless. Credits and the CC0 legal
code are in `plugins/vendor/sky/feature/assets/deep-space/`.

The HDR files are **not in the repository**. `plugins/vendor/sky/feature/build.rs`
downloads the pinned layers archive, verifies its SHA-256 and extracts the two
files into `OUT_DIR`, from where they are embedded (~23 MB in the binary).
Offline: set `PULSAR_SKY_ARCHIVE=<path to the zip>`; to skip, set
`PULSAR_SKY_SKIP_DOWNLOAD=1`. If the files are unavailable the build still
succeeds and `sky_texture::presets_available()` is false.

## Known limits

- Custom files are chosen by path (a text field), not the asset picker, and are
  not part of the SceneDB texture-asset/streaming system: the sky pass keeps its
  own single resident panorama.
- The panorama does not rotate with the sun; orient it by hand.
- No image-based lighting or ambient contribution from the panorama.
