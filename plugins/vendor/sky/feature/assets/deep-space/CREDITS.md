# Deep-Space Skybox (galaxy and nebula layers)

- Creator: Toolbench (https://toolbenchdev.com), made from scratch with Blender
  procedural shader nodes; no photos, scans or third-party assets.
- Source: https://toolbenchdev.com/gamedev/cc0-game-assets/deep-space-skybox/
- Download: https://toolbenchdev.com/downloads/deep-space-skybox-layers-1.0.zip
- License: CC0 1.0 Universal (public domain dedication). Commercial use,
  redistribution and modification are permitted; credit is not required.
  Legal code: `LICENSE-CC0-1.0.txt` (copied from the upstream archive).

Only the `galaxy` and `nebula` layers are used. They are separate from the
`stars` layer, so they are starless: the engine's procedural stars, shooting
stars and directional-light moon stay independently controllable on top.

The files are **not stored in this repository**. `build.rs` downloads the
archive, verifies its SHA-256 and extracts the two 4096x2048 Radiance HDR
files into the build directory, unmodified. They are embedded into the engine
binary at build time. Offline builds still compile; the built-in presets are
then reported as unavailable and user-supplied textures keep working.
