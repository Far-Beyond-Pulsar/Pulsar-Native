# Voxel Craft

A sandbox on a Helio voxel planet of 10 cm voxels: walk, jump, fly, dig and
build on a 16 km cube-sphere planet, among generated forests, bushes,
boulders, deserts, mountains and caves, with cottages, towers, ruins, wells
and pyramids to find, and a blocky character you can see in third person.
Every voxel's colour is procedural: nothing is textured.

```
cargo run -p helio-voxel-examples --release --bin voxel_craft
```

The native demo detects a connected OpenXR headset at startup, using the same
Vulkan bootstrap as `foliage_demo`. With a headset it renders stereo at the
runtime's recommended eye resolution and shows a side-by-side desktop mirror.
If OpenXR setup fails it logs the reason and starts the usual desktop game.
`VOXEL_CRAFT_VR=off` forces desktop mode; the screenshot tour also uses desktop.

### VR controls

Connect Oculus/Meta Link and start your chosen OpenXR runtime before launching
the demo. A successful connection logs `[XR] Voxel Craft ready`. The registered
OpenXR runtime determines whether SteamVR or Meta drives the headset.

| Controller input | Action |
|---|---|
| Head and hands | tracked view, pointing ray, selected block in the right hand |
| Left stick | head-relative movement with world collision |
| Right stick left/right | 30° snap turn; return to centre before turning again |
| Right stick up/down | next/previous hotbar block |
| Right / left trigger | hold to dig / press to build where the right hand points |
| A / right primary | jump; hold to ascend in flight |
| B / right secondary | toggle flight |
| X / left primary | pick the targeted block |
| Y / left secondary, or keyboard C | recenter room movement |
| Left grip | sneak; descend in flight |
| Right grip | sprint / fast flight |
| Keyboard R / Esc | respawn / quit |

Touch and Index use the face buttons above. WMR uses stick clicks for
primary actions and menu buttons for secondary actions; Vive uses trackpads
and their clicks. Simple controllers have pointing, triggers, and menu clicks;
keyboard movement remains available. If the pointing controller is untracked,
aim falls back to the headset gaze and the held block is hidden.

Room movement follows the planet's local gravity and uses the player's collision
solver. Snap turns pivot beneath the head. Looking and crouching remain tracked;
VR always uses first person. Inputs pause while the runtime dashboard has focus;
release the sticks and buttons to resume. Mirror resizing/minimizing does not
change the eye resolution or suspend headset rendering. VR uses native-resolution
FXAA without TSR or projection jitter. Runtime exit/loss ends the game cleanly.

### Desktop controls

| Input | Action |
|---|---|
| WASD, mouse | move, look |
| Space | jump (tap twice, or `F`, to fly; Space/Shift go up/down) |
| Ctrl | sprint / fast flight |
| LMB / RMB / MMB | dig a ball / build a half-metre block / pick the block under the crosshair |
| wheel, 1-9 | hotbar |
| V or F5 | first / third person |
| T | speed time up |
| R | respawn |
| Esc | release the mouse (Esc again quits) |

Environment: `VOXEL_CRAFT_QUALITY=performance|balanced|quality|native`,
`VOXEL_CRAFT_LOD` (cells per pixel, default 0.6), `VOXEL_CRAFT_DISTANCE`
(view distance in metres, default 12 000), `VOXEL_CRAFT_NO_CLOUDS=1` (a
clear sky).

`VOXEL_CRAFT_FRAMES=<n>` exits after `n` rendered frames, for a bounded desktop
or headset smoke check. `VOXEL_CRAFT_TIMINGS=<n>` logs GPU pass timings over `n`
draws (two eye draws per headset frame).

### Screenshot tour

`VOXEL_CRAFT_SHOTS=<dir>` is the example's render check: with nobody at the
keyboard, it holds the time of day at mid-morning, poses the camera at the
spawn in nine views (looking ahead, to the side, up and down; two third-person
views; from 45 m up looking down; behind; and from 400 m up looking out to
the horizon), saves each as `<dir>/shot_NN.png` and quits after the last.
A pose is shot once the planet pass reports the view settled and at least
`VOXEL_CRAFT_WAIT` frames have passed (default 120, for the temporal
anti-aliasing to converge), or after 900 frames regardless
(`VOXEL_CRAFT_WAIT_MAX`).

```
VOXEL_CRAFT_SHOTS=shots cargo run -p helio-voxel-examples --release --bin voxel_craft
```

* `VOXEL_CRAFT_POSES=<n>`: shoot only the first `n` poses.
* `VOXEL_CRAFT_WAIT=<frames>`: the minimum frames before each shot.
* `VOXEL_CRAFT_WAIT_MAX=<frames>`: the frames after which a pose is shot
  settled or not (default 900; with `VOXEL_CRAFT_WAIT=1`, the shot's log
  line gives the frames the view took to settle).
* `VOXEL_CRAFT_BURST=<n>`: save `n` consecutive frames a pose
  (`shot_PP_FFF.png`), the camera still: flicker shows as frame-to-frame
  differences. `python tools/flicker.py <dir>` measures it per pose (mean
  luminance change a frame, pixels changing by more than 2%, and each
  pixel's deviation over the burst, saved as `std_PP.npy`). It also lists
  event frames, where over 5% of the image changed at once: on NVIDIA
  Vulkan these are lost G-buffer writes (8x8 internal-pixel blocks showing
  the background, driver bug 6939141), not flicker; retake that burst.
* `VOXEL_CRAFT_TRUNK=1`: at frame 700, move the camera 4 m in front of the
  nearest tree log (within 15 m) and log that log and the voxels in
  front of it, so the next shot looks at a trunk (pair it with
  `VOXEL_CRAFT_POSES=1 VOXEL_CRAFT_WAIT=800`).
* `VOXEL_CRAFT_FLIGHT=<frames>`: instead of the poses, a flight for
  ghosting. Glowstone goes up ahead of the spawn (two columns on the
  ground, one block in the air in front of distant terrain); the camera
  settles 1 m up, then moves sideways at the fly speed
  (`VOXEL_CRAFT_FLIGHT_SPEED`, m/s, default 14) in steps of a 60 Hz frame,
  saving every frame (`flight_NNN.png`). Then it returns to every eighth of
  those places and saves each once still and converged
  (`still_NNN.png`, after `VOXEL_CRAFT_STILL_WAIT` frames, default 90).
  `python tools/ghosting.py <dir>` compares the pairs: light or dark left
  behind what moved (`trail`, `dark`: pixels off by over 0.1), and writes
  `ghost_NNN.png` (brighter than the still in red, darker in blue).
* `VOXEL_CRAFT_STROKE=<dig|build>:<radius m>:<stamps>[,...]`: instead of
  the poses, sculpt strokes for what edits cost the frame. Each stamps one
  brush a frame (a ball dug out, or a cube of cobblestone), dragged sideways
  over the ground 12 m ahead inside a 16 m query focus, then waits for the
  world to settle; a `STROKE` log line gives the frame times over the
  stamps and the settle, the world's part of the frames (snapshots,
  queries, the player) and the longest stamp call. For example
  `dig:4:40,build:6:40,dig:0.35:60,build:0.25:60`.
* `VOXEL_CRAFT_QUALITY=fxaa`: no TSR (FXAA at native resolution), to tell
  what is temporal.

## Layout

| Path | What |
|---|---|
| `main.rs`, `app.rs` | entry point; the winit window and its events |
| `game.rs` | game state and one frame (input, player, world, draw, title HUD) |
| `gfx.rs` | device, surface, scene (sun, atmosphere, post), the voxel planet pass, frame drawing |
| `config.rs` | tunables |
| `blocks.rs` | the blocks: voxel material ids, colours and patches, light through and given off |
| `palette.rs` | the planet's appearance from the blocks, and the block-in-hand materials |
| `terrain/` | the planet recipe (layer stack and material rules), biomes, hash noise |
| `world/` | the edit worker thread, snapshots, block queries; `decorate.rs` is the decoration: tree, cactus and structure stamps (trees, bushes, boulders and cacti grown in this world's blocks by the engine's `helio_pass_voxel_planet::flora`) and the spawns that place them |
| `structures/` | blueprints (stamps of this game's blocks, a block a metre): cottage, tower, ruins, well, pyramid |
| `player/` | movement and collision on a sphere (stepping up ledges), digging and building, the character |
| `input.rs` | keyboard and mouse state |
| `xr/` | OpenXR frame lifecycle, controller actions, planetary rig, spectator mirror |

## How it fits together

* **Terrain** is the voxel planet's layered generator (`helio.terrain`), set
  up as data in `terrain/recipe.rs`. Block ids are the planet's material ids:
  the generator's own (grass, dirt, stone, sand, snow, gravel, ...) and ids 22
  to 31 for this game's (birch, spruce and jungle wood and leaves, cactus,
  glowstone, stone bricks, dark oak planks).
* **Colour is procedural.** Each block is a voxel material with a colour,
  and the natural ones dry, middle and lush patch colours
  (`Block::look`); the planet pass varies each voxel's pigment and each
  region's patches (`TerrainAppearance::detail`), so a meadow, a bark or a
  rock face reads as voxels of near colours, as in Teardown. Leaves pass a
  third of the light through for global illumination; glowstone lights its
  surroundings.
* **Edits** (the player's) are brushes applied by a worker thread to the
  authoritative planet, which publishes immutable snapshots
  (`world/worker.rs`): a dig takes a 35 cm ball out, a build lays a
  half-metre cube on the block lattice. The renderer draws the latest
  snapshot; physics reads it plus the player's pending edits, so a dig or a
  build is felt at once.
* **Decoration** is the voxel planet's *features*
  (`helio_pass_voxel_planet::features`, `world/decorate.rs`): a library of
  stamps of shapes (tree variants grown per species as tapered capsules,
  sparse leaf balls and cones; bushes, boulders, cacti; structures with
  their foundations) and spawns placing them by biome, height, flatness and
  forest fields. They are part of the planet's recipe and evaluated like its
  terrain, on the GPU for drawing and on the CPU for physics and picks:
  nothing streams around the player, memory does not grow with the area,
  forests show to the horizon as their canopies, and what the player digs
  or builds applies over them.

In VR, the application waits for OpenXR before simulation and samples head and
controller poses at that frame's predicted display time. It supplies each eye's
absolute position to the voxel source and each asymmetric projection to the
normal render graph. Residency follows the head centre so eye separation does
not become artificial motion. Floating-origin subtraction happens in double
precision before constructing GPU transforms. Each begun OpenXR frame is ended,
including frames skipped for tracking loss and frames whose rendering fails.
