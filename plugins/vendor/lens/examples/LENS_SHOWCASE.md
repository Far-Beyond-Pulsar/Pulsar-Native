# Helio lens showcase

`lens_showcase` renders a deterministic optical test scene through Helio's production render graph and saves one 1280×720 PNG per configuration. It is intended for the lens simulation article and for visual review when lens parameters change.

From the Pulsar-Native repository root, run:

```powershell
cargo run --manifest-path crates/renderer/helio/Cargo.toml -p examples --bin lens_showcase -- --all --output-dir "C:\path\to\blog\public\posts\2026-10-20-through-the-glass-lens-simulation\assets\lens-showcase"
```

Without `--output-dir`, captures go into `lens-showcase` in the current directory. Existing files with matching names are replaced. The default command without `--all` launches the interactive scene; `--probe` prints the GPU adapter and render-graph information.

## Captured configurations

- **Exposure and focus:** `baseline`, `no-lens`, `wide-open`, `shallow-depth`, `stopped-down`, `autofocus`, `tilt-focus`, `field-curvature`, `sweet-spot`
- **Pupil and bokeh:** `anamorphic`, `anamorphic-133`, `anamorphic-200`, `swirly-bokeh`, `cats-eye`, `spherical-rim`, `spherical-soft`, `onion-rings`, `apodization`, `central-obstruction`, `iris-rounded-blades`, `iris-texture-star`, `iris-texture-heart`, `iris-texture-triangle`
- **Split focus:** `split-diopter`, `split-horizontal`, `split-vertical`, `split-diagonal`, `split-multi-ellipse`, `split-radial-band`, `split-polygon`, `split-seam`
- **Color and mapping:** `longitudinal-ca`, `lateral-ca`, `barrel`, `pincushion`, `division-distortion`, `anamorphic-distortion`, `anamorphic-squeeze`, `lens-shift`, `keystone`, `fisheye`, `projection-equisolid`, `projection-orthographic`, `projection-stereographic`, `projection-kannala-brandt`, `panini`, `cylindrical`, `focus-breathing`
- **Filters and transmission:** `natural-vignette`, `optical-vignette`, `circular-hood`, `matte-box`, `nd-filter`, `graduated-nd`, `color-filter`, `t-stop`, `flat-port`, `dome-port`, `prism-line`, `prism-ring`
- **Aberrations and image-space elements:** `spherical-glow`, `coma-astigmatism`, `corner-softness`, `diffusion`, `diffusion-tail`, `pro-mist`, `black-pro-mist`, `glint`, `glimmerglass`, `fog-diffusion`, `halation`, `halation-tint`, `star-filter`, `water-drops`, `water-drops-transformed`, `smear`, `dirt`
- **Prescription and custom hook:** `prescription-thin-lens`, `prescription-cooke`, `prescription-double-gauss`, `user-hook`

The scene combines bright point sources at several depths, an emissive color-and-edge chart, a focused foreground subject, and a neutral room. It disables the separate lens-flare post effect so captures isolate the camera lens component. The custom-hook capture supplies example WGSL for ray mapping, transmission, and focus. The prescription cases use the bundled prescription bank and aperture-mask resources.

These are illustrative outputs from the current renderer, not calibrated optical benchmarks. Several effects are subtle at the selected settings; compare each capture with `baseline` before drawing conclusions about their numerical accuracy.
