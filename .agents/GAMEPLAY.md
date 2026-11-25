# Gameplay: Build & Run, role spawning, the player character

How the editor's Build & Run packages and launches a game, how a level's gameplay roles become
actors and what their scripts hear, the default player character, and what is known to be missing.
The trait and role machinery is `.agents/TRAITS.md` ("What runs when a level plays"); the component
lifecycle is `.agents/COMPONENT_LIFECYCLE_PLAN.md`; the MCP tools that drive and observe all of it are
`.agents/MCP.md`.

## Build & Run, as built

The toolbar's Build split button runs the selected build configuration
(`.pulsar/build_configurations.toml`, `engine_state::build_config`; `Build & Run` is one of the defaults)
through `ui_build::run_configuration`. Its steps, in order, each part of one editor task (`Build: <name>`,
log = the build's output, shown live in the Build Dashboard):

1. **Regenerate core project files** (`engine_backend::services::ensure_core_bootstrap`): the game
   crate's `Cargo.toml` (the enabled plugins' root crates, `pulsar_game`, the baked `[patch]` table), `src/`
   glue, `scripts/`. It also aligns the game's `Cargo.lock` with the engine's git pins (below).
2. **Pack content** (`pulsar_package::pack_content`, through the `pulsar_content::ContentPacker`
   registration): compile every class with each language plugin (`ScriptLanguage::compile_project`),
   verify and link every module against the engine's natives and the project's declared events, cook
   levels and classes, snapshot the plugins' folders (`plugins/<Name>/`, the Gameplay plugin's
   `PlayerCharacter.class`), and write the pak set to `<target dir>/<profile>/Content/` next to where
   the game binary will be.
3. **Build** `cargo build` of the game crate (`CARGO_TARGET_DIR` and `.cargo/config.toml` are honoured:
   the output folder is Cargo's own answer).
4. **Run**: `cargo run` per multiplayer instance (`launch_and_monitor`), a `Game: Run` editor task. Every
   line the game writes on stdout and stderr is a line of that task's log (and so of the live log, the
   Build Dashboard and the MCP log tools); stderr's tail is kept for a crash report
   (`.pulsar/crash-reports/`). The Stop button and `build_stop` kill the game's whole process tree.

Failures are not silent: a failed step shows the failure dialog (one card per compiler error) and is a
**finding** in the Problems panel (`ui_build/src/findings.rs`: sources `Build` and `Game`, replaced by
the next build or run): a failed build, a game that could not start, a game that crashed. The task log
keeps a multi-line error as one row per line (`editor_task_queue::TaskLog::push`), so the dashboard shows
all of it.

### What was broken (found by running it)

- **The pack step rejected a plugin's mounted class** ("has graph_save.json source but no compiled
  module" for `plugins/Gameplay/classes/PlayerCharacter.class`): a plugin library has its **own copy of
  `engine_fs`**, with none of the host's project mounts, so the Blueprint plugin's class walk and the
  input plugin's action scan did not see `plugins/<Name>/`. Fix: whatever lists a project's files from
  a root mounts it first (`pulsar_class::find_class_dirs`, the Blueprint plugin's `project_class_dirs`,
  the input plugin's `Library::scan`), idempotently, as `pulsar_traits`' index already did.
  `plugins/installed/tests/gameplay_defaults.rs::the_mounted_class_is_found_and_compiled_without_the_project_being_mounted_first`.
- **The game's `cargo build` failed in `helio-core`** (`no method named sizes found for struct
  wgpu::Blas`, `request_rebuild`): the game has its own `Cargo.lock`, kept from an earlier generation,
  and the baked `[patch.crates-io]` named the wgpu fork by *branch*, so the game resolved another commit
  than the engine's. Fix (`engine_backend/src/services/git_pins.rs`, `build.rs`): the baked manifest
  carries the commit the engine's own `Cargo.lock` pins (`rev`, not `branch`) for every git source named
  by branch, and an existing game lockfile has every git package the engine also pins moved to the
  engine's commit (`Moved git packages of the game's Cargo.lock to the engine's pinned commits`).
  Neither bumps a rev or repoints a branch. Tests in `git_pins` and
  `core_project_builder::tests::baked_git_patches_carry_the_engines_pinned_commit`.
- **A shipped game spawned no roles**: `Game Mode class 'graph_save' is not in this project`. A role's
  class was guessed from its asset path by asking the *language registry* which file names are class
  markers; a game links no language, so `graph_save.json` was not a marker and the class became its
  file stem. Every test linked the languages (through `installed_plugins`), which hid it. Fix: the role's
  class is the class whose **folder holds the asset**, found in the class registry (the content's class
  index): `ClassRegistry::by_asset`, used by `ScriptDriver::resolve_role_class`. Test:
  `pulsar_class::registry::tests::an_asset_path_names_the_class_whose_folder_holds_it`.
- `plugin_builder --only <id>` removed every other plugin library from the install (it pruned against
  the subset it built): an editor with four plugins of thirty-four after a one-plugin rebuild. It now
  prunes against every buildable plugin.

## Roles in a session

`ScriptDriver::start_roles` spawns each role the level names as an entity of its class through the world
command path (`TRAITS.md`). Verified in the packaged game from the live game log:

```
Game Mode spawned class=RoleProbeMode
Game State spawned class=RoleProbeState
Player Controller spawned class=RoleProbePad
Player Character spawned class=PlayerCharacter
Starting script instances (begin_play) instances=3   (then 1 for the character)
```

- **Where the character spawns**: `world_settings.player_start` (`[x, y, z]`, metres; the other roles
  are not in the world's space and spawn at the origin). No key: the origin. The key is a plain level
  setting that a level file carries untouched; the editor has no field for it yet.
- **Session scopes**: packaged and headless games, and Play-in-Editor, all run `start_roles` in the
  driver's first script phase; the editor's own edit mode spawns no roles (a level in the editor is
  data; roles are session objects, never saved).
- **Events a role's script gets**: the trait methods (`GameMode.on_game_start`, ...) through the one
  dispatcher, `begin_play`, `tick` every frame and `end_play` at the session's end (Blueprint nodes
  `begin_play`, `on_tick`, `on_end_play`), and for the possessed pawn the `input::<Action>.<phase>`
  events on its own channel.

### The role probe fixture

`crates/core/pulsar_package/tests/fixtures/role_game` (a level of no objects whose roles name three
probe Blueprints and the Gameplay plugin's character) and `tests/role_spawn.rs`: package it through the
real path, run the packaged content headless the way a game's `main()` does, and check the probes'
counters (hooks heard, `begin_play`, ticks), the four role entities, an `IA_Jump` event reaching the
pawn's Blueprint, and that no role entity outlives the session. The probes are written by a script
(`mkprobe.js`, in this change's scratch notes: counters in class variables, one `log_info` per event).

## The default player character

`plugins/vendor/gameplay` ships `PlayerCharacter.class` (mounted read-only at
`plugins/Gameplay/classes/`): a Blueprint implementing the Player Character trait, on an object with
`CharacterComponent`, `CharacterControllerComponent` (the capsule: `radius`, `height`, `skin_width`,
`step_height`, `slope_limit_degrees`, `snap_to_ground`, `gravity_scale`, all reflected),
`PlayerInputComponent` (mapping `IMC_Default`) and `CameraComponent`. Its graph is generated by
`plugins/installed/tests/regenerate_player_character_graph.rs` (`REGENERATE_PLAYER_CHARACTER=1`).

- **Move / jump / sprint**: `IA_Move`, `IA_Jump`, `IA_Sprint` drive `CharacterComponent` (`set_move`,
  `request_jump`, `walk_speed`); its `pulsar.character.move` system turns them into the controller's
  desired velocity and vertical speed.
- **Look and the control rotation**: `IA_Look` feeds the character's **control rotation** and nothing else
  (`CharacterComponent.add_control_yaw` / `add_control_pitch`; the Blueprint has no axis maths). The control
  rotation (`control_yaw`, `control_pitch`, `control_roll`, pitch limits `view_pitch_min` / `view_pitch_max`) is
  what the camera and the body read. It lives on `CharacterComponent` (the character plugin), not on a
  separate controller object: the Player Controller role is a Blueprint entity at the origin, so a
  rotation owned by it would need a pawn-to-controller link the roles system does not have yet.
- **Movement is relative to the control yaw**: `set_move(x, y)` (right, forward) is walked along the yaw-only
  right and forward of the control rotation (`movement_direction`: yaw 0 looks along -Z, +X is right, a
  positive yaw turns left), so looking up or down never tilts the walk. `add_movement_input(x, y, z, scale)`
  adds a world direction for one frame.
- **Rotation options** (reflected, in the Properties panel and on the Blueprint node): `use_controller_rotation_yaw`
  / `_pitch` / `_roll` (the body takes the control rotation on that axis), `orient_rotation_to_movement` with
  `rotation_rate` (degrees per second; the body turns the short way to face its walk, the camera keeps the
  control rotation). Demo defaults, third-person style: orient on, controller yaw off. First-person / strafing
  preset: `use_controller_rotation_yaw` on, `orient_rotation_to_movement` off. The character writes a changed rotation
  only (a transform write makes the physics step take the body's position from it).
- **Camera follows the control rotation**: `CameraComponent.use_pawn_control_rotation` (on in the shipped character's
  prefab) takes the view's yaw from `control_yaw_deg` (written by the character each frame) and its pitch from
  `pitch_deg` (the control pitch), ignoring the owner's own rotation. Off: the owner's rotation and the camera's
  own pitch, as before. There is no separate spring-arm component: `arm_length` on the camera is the boom, and
  inherits nothing from the body (so no inherit pitch/yaw/roll options yet).
- Not built: Blueprint helper nodes (Get Control Rotation, Get Actor Forward/Right Vector, make rotation from yaw
  only): the properties are readable and settable from a graph, but there are no dedicated nodes.
- **The camera rig** (`helio_component::CameraComponent`): `eye_offset` (owner space; the character's
  is 0.7 m above its capsule centre), `arm_length` (0: first person, more: third person behind the eye),
  `pitch_deg` and its limits, all reflected properties. The host builds the frame's camera from
  the owner's pose and this rig (`pulsar_game::camera_selection::render_camera`).

### Falling through the map

The capsule exists (`CharacterControllerComponent`) and the physics plugin's character path runs it, but
the Bistro level had **no colliders at all**, and the plugin's mesh collider shapes (`ConvexHull`,
`TriangleMesh`) are declared and **not simulated** (`physics_plugin/src/components/collider.rs`:
"mesh colliders are not simulated yet; skipping"). So the project's default level got a static
**box collider as the ground** (object `Ground Collider`, a `ColliderComponent` box of 800 x 1 x 800 m whose
top is at y = 0, where the Bistro's street lamps stand) and `player_start` `[0, 1.2, 0]`.
This is a flat floor, not the street's real surface: steps and kerbs are not walked on. The proper fix is
mesh collision in the physics plugin (a trimesh from the `.mesh` asset, per static mesh), listed below.

### Input latency

A probe is always on (`pulsar_game::latency`; at most four log lines a second, target `pulsar_game::latency`): one
sample follows the oldest unshown input through ten stamps and nine stages: the OS event (stamped on the raw-input
thread the moment the OS delivers it, see "Input is sampled at the OS" below),
`wait_for_tick` (to the start of the next tick), `input_and_systems` (the schedule and component phase: input
drained, the character, the physics hand-off), `scripts`, `scene_write` (the change window closes),
`wait_for_frame` (to the frame's read of the world), `sample` (the scene step and the camera), `extract_upload`
(scene flush and camera), `render_submit` (recording and submitting the graph) and `present`. While the profiler records, every sample is also
published as scopes on the `Input latency` track (root `Latency::input_to_present`, one child per stage), so it is in
the same recording as everything else: **`profile_latency`** returns each stage's mean, p50, p95, p99 and worst and
its share (MCP.md, "Profiling"). `latency::summary()` is the mean and worst of the last 240.

The stamp after `present` is the earliest the display can show the frame: the present queue (`max_frame_latency`,
1) and the scan-out come after it. Not measured: the monitor. The OS-to-handler time (the winit loop answers events
between frames, so an event that arrives during a 7 ms frame waits for it) used to be invisible, because the first
stamp was taken when the handler ran; it is now inside `wait_for_tick`, and measured on its own as
`Latency::os_to_handler` (the time raw input waited for the event loop: mean 3 to 9 ms in captures, 9 ms when the
frame was 15 ms), so a chain's total is not comparable with one recorded before it existed (the new total contains a
wait the old one did not).

Measured in the packaged release game (RTX 4090, 144 Hz display, Bistro level, mouse turning, the same capture
each time):

| | mean | p95 | wait_for_tick | wait_for_frame | render_submit |
|---|---|---|---|---|---|
| before (60 Hz tick, blocking GPU timestamp readback) | 29.6 ms | 38.9 ms | 9.9 ms | 5.8 ms | 12.7 ms |
| readback no longer blocks | 19.8 ms | 29.8 ms | 7.7 ms | 4.6 ms | 6.6 ms |
| tick follows the display (`simulation.tick_rate_hz`) | 14.5 ms | 16.8 ms | 3.3 ms | 3.5 ms | 6.0 ms |

What moved it: the render graph read the GPU timestamps with `device.poll(wait_indefinitely)` every frame whenever it
owned its device (the packaged game), so the CPU waited for the whole GPU frame before starting the next; and a
fixed 60 Hz tick made every input wait for the next tick (up to 16.7 ms on a 144 Hz display). Earlier work:
the shipped `IA_Look` had a `smooth` modifier (a 55 ms time constant), an exponent 1.6 curve and an actuation
threshold that dropped slow movement; it now has no modifiers (`gameplay_defaults::the_look_action_adds_no_delay_and_drops_no_slow_movement`).
What remains of the 14.5 ms, in order: `render_submit` 6 ms (the Shadow pass's command-buffer finish, 3.3 ms of
it: see "Where a frame goes" below), `wait_for_frame` 3.5 ms and `wait_for_tick` 3.3 ms (the loops run at
different phases; a late-latched look, applying the newest mouse delta to the view when the frame starts, would
remove both for the camera), and the display.

### Input is sampled at the OS (`pointer.rs`, `frame_pacer.rs`)

- **A raw-input thread** (`pulsar-raw-input`, Windows; `pulsar_game::pointer`) owns this process's raw mouse and
  keyboard registration (a message-only window, `RIDEV_INPUTSINK`; direct `user32` declarations so no `windows`
  feature set, and with it no feature hash, changes). It adds mouse motion to a lock-free accumulator and stamps the
  arrival on the latency probe's clock. Because a process has one raw registration per device, winit no longer sends
  `DeviceEvent::MouseMotion` while it runs: the free camera takes the same motion from the sampler
  (`take_view_motion`, drained in `about_to_wait`), where the wait of the event loop is measured too.
- **The sample point.** The tick drains the accumulator where it reads input (`TickLoop::sample_pointer`, ahead of the
  after-input flush) and publishes one `MouseMoved` with the sum: as fresh as the tick, one publish per tick however
  fast the mouse reports. Non-Windows builds feed the same accumulator from the event loop's device events.
- **The frame reads the world after it waits for the display.** `GameWindow::render` acquires the surface first
  (with a present queue of one the acquire is the wait for the display), then steps the scene and takes the camera
  (`Game::step_scene`, `Game::camera_for_frame`), right before `prepare_frame` uploads it. It used to read both
  before the acquire, so the camera was as old as the vsync wait when it was drawn.
- **The tick is placed just before that read.** Tick `k` is due at `start + k * step` and may run any time within one
  step of it; `FramePacer::next_start` picks the moment that ends it just before the frame's predicted read
  (last read plus the smoothed frame period, minus the tick's high-water cost, the prediction's own error and 0.2 ms).
  The due times never drift (the simulation runs at its rate on average; before, each tick slept `step - elapsed` and
  the game ran slower than real time by the tick's overhead), and a loop that fell four steps behind starts over. When
  frames stop (hidden, occluded) or there is none yet, the tick keeps its timer.
- **World age at the frame's read** (the time from the end of the last tick to `Game::camera_for_frame`, same machine,
  8 ms frames): mean 3.3 ms before, 1.8 ms after (p50 3.3 to 1.3 ms); at 15 ms frames under GPU contention 3.3 to 2.7 ms.
  The p95 stays near 6 ms (a mispredicted frame). Input to present, same capture: 16.3 ms mean before (its first stamp
  at the handler), 14.4 ms after (its first stamp at the OS): `wait_for_frame` 4.9 to 2.5 ms.
- **The late-latched look** (`late_look.rs`, `pointer.rs`, `camera_selection::turned_rig`). The view shows what the
  last tick wrote plus the mouse motion no tick has consumed yet, applied when the frame reads the camera, right before
  its uniforms. The frame never writes the world: it asks the plugins what the pending motion turns the view by and turns
  the camera rig; the next tick drains the same motion and applies it through its own look path, so it is counted once.
  - **Contract** (`engine_backend::scene::game_hooks`): `ControlRotationRegistration` (the character plugin: which
    action turns the control rotation, `ViewTurn` = degrees of yaw and pitch for an action value and the pitch limits)
    and `LookSourceRegistration` (the input plugin: the value an action takes for pointer motion alone). The kernel
    names neither plugin. The handler calls `CharacterComponent::add_look(x, y)` (the shipped `IA_Look` Blueprint does);
    the character owns `look_degrees_per_unit` and the limits, so the tick and the frame use one rule (`ViewTurn::pitched`).
    A character is late-looked only after a graph drove it with `add_look` (`look_driven`): a project whose own class
    maps the look another way keeps the tick's view. The input plugin tells a value ahead only when the mapping is linear
    (scale, negate, swizzle on the entry and the action), no trigger or chord shapes it and no other device (a stick)
    moves the action; otherwise the frame shows the tick's view. The rig is turned only when the camera follows the
    control rotation (`use_pawn_control_rotation`); the body (`use_controller_rotation_yaw`, `orient_rotation_to_movement`)
    is the tick's, as before.
  - **Exactly once**: `PointerSampler` keeps monotonic totals; the tick takes the difference at its sample point and
    brackets itself with a sequence number (`begin_tick` / `end_tick`, odd while it runs). What no finished tick has
    applied is `total - applied`; the frame reads it with the world held and re-reads the world when the sequence changed
    (`read_with_pending_motion`: waits at most 2 ms for a running tick, three attempts, then shows the tick's view). A motion
    is never counted by both the frame and the world, and never by neither.
  - **Tests**: the pending turn equals the tick's write in the same units (`character`: wrap, each pitch limit, inverted
    pitch, a motion split across a tick; `camera_selection`: a turned pawn camera, with an arm, equals the camera the tick
    writes), `InputSystem::pointer_value` equals the value the evaluation reports and is `None` for smoothing, a dead zone
    or a pushed stick, and the sampler/tick protocol (`late_look::tests`).
  - **Setting**: `Pulsar/project.json` `simulation.late_look` (default on). A packaged game reads no editor or project
    settings files, only `Pulsar/project.json` (`pulsar_content::ProjectSettings`): `window.present_mode` lives there too.
  - **Measured** (packaged game, 165 Hz display, same scenario each time): the mouse look reaches `present` in a mean of
    6.1 ms (p95 9.0) from the OS, against 10.6 ms (p95 16.8) for the tick path of the same build and 13.7 ms (p95 18.7)
    for the build before. `Latency::look_to_present` is its own root on the `Input latency` track. What remains of the 6.1:
    about 2 ms until the frame reads the camera (the frame cadence), 3.8 ms `render_submit`, 0.4 ms extract and present; the
    display's scan-out is not measured. Keys and the mouse buttons still go through the event loop (`os_to_handler` 3.7 ms
    mean) and the tick (`wait_for_tick` 3.5 ms).
  - **Not done**: interpolating the character's pose between ticks (the tick follows the display, so a frame sees a
    new pose every frame; physics steps at 60 Hz and its pose changes every 2.75 frames at 165 Hz: interpolating it
    renders it a step behind, which trades latency for smoothness and belongs to the physics plugin), and patching the camera
    again after the frame is recorded (the 4 ms between the frame's read and its submit; that is a Helio camera late-update).

### Where a frame goes (measured with the profiler MCP tools)

The same capture, `profile_top_scopes` / `profile_threads` / `profile_gpu` on the packaged game idle, 144 Hz display:

- Frame period 7.5 ms (134 fps; 18.6 ms / 54 fps before the readback fix), p99 10.3 ms. The frame thread is
  CPU-bound, not GPU-bound: the GPU frame is 4.1 ms mean (p95 6.9): Atmosphere 1.2 ms, Shadow 0.5 + static layer 0.3,
  DeferredLight 0.5, ObjectBatch 0.3, the rest under 0.15 each.
- 4.3 ms of the 7.1 ms is `RenderGraph: wait for encoder-finish threads`. With `HELIO_FINISH_BREAKDOWN=1` the finish of
  one command buffer dominates: **`Shadow` 3.3 ms** (recording it takes 0.3 ms), then `MeshHitSurfaces gi_probe` 0.7 and
  `VgBvh` 0.6. The Shadow pass walks every serviced shadow face every frame and records a viewport, a scissor, two
  pipeline switches, three bind groups and two draws per face and layer (`meshes/feature/shadow/src/lib.rs`, the loop in
  `execute`); wgpu validates all of it in `CommandEncoder::finish`, which is serial per segment.
- The tick thread costs 0.3 ms per tick; physics 0.04 ms per step. Neither is a factor.

### Streaming a level in (measured)

First 14 s of the packaged game, same machine, before and after (`profile_frame_times`, `profile_top_scopes`):

| | frames in 14 s | mean frame | p95 | p99 | worst frame |
|---|---|---|---|---|---|
| before | 77 | 169 ms | 592 ms | 1523 ms | 1713 ms |
| after | 1382 | 9.3 ms | 20.8 ms | 39 ms | 76 ms |

Main-thread time in `scene::pump_assets` over the 14 s: 9.7 s before (9.0 s of it material binds), 1.2 s after (material binds 1.0 s, of which texture uploads 0.44 s). The cause was not a missing worker: textures were already cooked on loader threads. The material bind on the frame
thread (`Assets::apply_bindings material`, 9.0 s of the 13.7 s) did the rest: (1) a graph's roughness and normal
maps were cooked as colour on the loader and then again for their real use on the frame thread; (2) a cooked form the
residency cache had since evicted was read from the derived cache on the frame thread (loads outran the frame; the
cache is a budgeted LRU); (3) the frame bound up to 8 assets per column walk and read the clock only between walks,
so one pump could cost 600 ms; (4) the tick thread pumped assets too, taking the scene write lock from the frame, and the
windowed frame pumped twice. Now: the loader cooks for the use and pins the result until the upload
(`LoadedTexture::pinned_for`); loads in flight count what has loaded and not bound; a pump binds as many assets
per walk as the time budget holds at the cost seen so far; a render loop is the only pumper; loader threads are half
the cores. The GPU upload is staged and budgeted now (`Texture::upload_cooked` was 0.8 ms mean, 4 to 27 ms max; see issue 3 of the
log below). The graph build is a worker's now (issue 2 of the log below); what remains on the frame thread while streaming is
`create_window_renderer` before the first frame. Derived cache: `install_derived_cache` (`helio_bridge`) points it at `<project>/.pulsar/cache/derived`
with the `streaming.derived_cache_mb` budget (default 32768); a packaged game's project path is its extracted
asset cache (`%TEMP%/pulsar-asset-cache/<hash>`), which persists between launches.

### Performance issues log (open, with the evidence)

Each was seen in a recording of the packaged game on the Bistro level (`profile_*` tools, `.agents/MCP.md`):

1. **The Shadow pass cost 3.3 ms of CPU in `CommandEncoder::finish`** (GPU 0.5 ms): fixed to 0.7 ms while faces are
   serviced and about nothing when none are. The cost was not the commands (cutting them to a third changed nothing): wgpu
   validates every texture view of a bound group once per render pass that uses it (`TextureViewBindGroupState`, one merge
   per entry, about 80 ns each), and the material texture table is 4096 views (the unused slots are one fallback view
   repeated). The shadow pass opens two such passes (static and dynamic layer) every frame, whatever they draw. Now
   (`meshes/feature/shadow`): the faces to visit come from the GPU's pending-dirty bits read back without waiting (they
   persist until a face is serviced, so the two frames of lag lose nothing; `DIRTY_LINGER_FRAMES` keeps a face that keeps
   being dirtied in the set) plus what the CPU knows (never drawn, light or static caster changed); a frame with none opens
   no pass; one draw per layer clears every serviced face's pages (`depth_clear.wgsl`) and one dispatch retires them
   (`retire.wgsl`). The sun's cascades were redrawn, both layers, every frame of an idle scene (matrix hash changed with
   the TAA jitter, an unquantised radius and a free eye along the light): they fit the unjittered frustum with a stepped
   radius and a snapped eye (`shadow_matrices.wgsl`), and are redrawn in about a third of idle frames (camera noise
   crossing a texel). **Fixed at the table, for every pass that binds it:** a device that binds arrays partially
   (`PARTIALLY_BOUND_BINDING_ARRAY`, Vulkan and D3D12, requested with the texture arrays) binds only the table's live
   prefix: `MaterialTextureTable::set_live_slots` is fed the texture store's slot high-water mark
   (`TextureStore::slot_count`, which only grows), `bindings()` hands out that many views, and
   `MaterialBindingConfig::append_bind_group_entries` takes a prefix of a binding array (the layout and the shaders keep
   the capacity). `RenderGraph: encoder.finish (segment)` in the 20 s walk/turn/jump script: 14.8 s over 13741 calls
   (1.08 ms each) to 6.7 s over 15356 calls (0.44 ms) in the first A/B capture, 8.2 s / 12861 (0.63 ms) in the next on a
   busier machine; `wait for encoder-finish threads` 1.74 to 0.80 ms (1.03 in the busier one). A device without the feature
   binds the whole table as before. **Open hazard:** a graph material's generated WGSL indexes
   `scene_textures[material.tex_*]` without a guard, and an unassigned slot is `u32::MAX`, which the shader's bounds
   handling clamps to the last element of the array; that element used to be a bound white fallback and is now unbound.
   The bistro level showed nothing wrong, but a graph material sampling before its texture resolves reads an unbound
   descriptor. The proper fix is a guaranteed white element inside the prefix (slot 0 of the texture store reserved) and a
   graph lowering that maps `MATERIAL_NO_TEXTURE` to it (the WebGPU lowering already does: `default: return fallback`).
2. **First use of a pass compiled its pipeline on the frame thread.** Material variants of the G-buffer, forward and
   transparent drawers (63 to 140 ms hitches) are fixed: they compile on the task system
   (`helio_core::compile::Variants`) and draw with the default material of the same raster state until ready.
   **Fixed: the graph build is off the frame thread** (`helio::Renderer::set_pipeline`, `pipeline_control.rs`):
   `IntoGraph: Send`, the build reads an owned `BuildInputs`, runs on a `helio_tasks::submit` worker under the same
   panic and GPU-validation guard (`guard_build`; an error scope belongs to the thread that pushed it), and the frame
   boundary (`advance_pipeline`, never waits) installs the graph when it has ended. While it builds, a renderer that
   draws no pipeline shows the status screen "Building the render pipeline" (`PipelineStatus::Building`), and one that
   renders a pipeline keeps drawing it (`pipeline_building()` says one is on its way). `apply_pending_pipeline()` is the
   same path plus a wait, for a builder, a test or a tool. A resize during the build is applied to the new graph in place;
   a configuration rebuild waits for the build. The host choice is `SceneRendererOptions::pipeline_build`: the windowed
   game uses `PipelineBuildMode::Background` and follows the outcome in `SceneRenderer::follow_pipeline` (logged once,
   when the renderer is done with it); the editor viewport, tools and tests keep `Blocking` (they read the first frame).
   `Helio: apply pipeline` in the first 14 s of the game: 827 ms over 2 calls (max 820 ms) before, 16 ms (max 8.5 ms) after.
   `create_window_renderer` still builds the renderer's device-side state on the frame thread; what it spent on the
   pipeline is gone from it (see the measured table in the round report).
3. **Fixed (mostly): a texture's GPU copy is staged and written under a per-frame budget** (`TextureStore::register_mips_staged`
   / `advance_uploads`, `streaming.texture_upload_budget_kb` 4096 and `_ms` 1, nearest first). `Texture::resident_slot`
   in the first 14 s: 569 calls, 1145 ms, max 27.5 ms before; 90 ms and max 14 ms after (mean 2.0 to 0.16 ms), and the
   upload itself is `Assets::advance_texture_uploads`, mean 0.53 ms, max 2.7 ms. Ranking: a load's score is dropped when it completes, so every staged texture was unranked and written in the order it was staged, whatever "nearest first" said; `advance_texture_uploads` now scores the staged ones with the host's priority function each call (rescored every few pumps), and the function takes the view cone (`World::set_asset_viewer`, `scene::asset_priority`: distance, stretched up to 4x by how far outside the cone that holds the frustum a thing lies). Bounds are not known to the ranking, so screen size is the distance's. Not measured on a screen: the capture does not turn far enough to show the order. Still open: a bind still waits for its cook when the loader has not
   pinned it (never seen in the captures: `Texture::cooked_for` had no calls).
4. **Fixed (mostly): `level_hydration::step` takes up to 4.9 ms against a 4 ms budget** (27 to 40 ms before). Validation and the
   property pass are one object / 128 objects a unit now, and the one object that took 29 ms was
   `Cable Conn A & Config`: its `CableComponent` resolves its tile texture through the project file index, whose first
   open scans the project, on the frame thread. The prepare worker opens the index now (`level-prepare: project file
   index`), and any object whose components take over 8 ms is named in a warning. What stays indivisible is one
   object's `attach_records`; a class whose hydrate does real work must move it to its prepare half. The legacy
   world-space transform conversion (levels older than parent-relative transforms) is still one unit.
5. **Fixed: input is read at the OS and consumed at the tick**, the frame reads the world after the display wait and
   the tick is placed just before it ("Input is sampled at the OS" above), and the frame turns the view by the mouse motion no tick has applied yet (the late look, same section). Open: keys and mouse buttons still wait for the event loop (`os_to_handler` 3.7 ms mean) and the tick.
8. **Fixed: a static mesh's geometry is held once per asset.** `LoadedMesh` holds `vertices`/`indices` as `Arc<[T]>` and
   `StaticMeshComponent` fields of the same type: `bind_mesh_asset` clones two `Arc`s (SceneDB's `SceneStore` derive takes
   `Arc<[T]>` as the variable-length field it takes `Vec<T>` for, `as_vec_elem_type`; the interned pool reads the elements
   only when a content id first arrives; `engine_class_derive`'s own field check agrees). `Assets::apply_bindings mesh` in the
   first 14 s: 404 ms total, max 57 ms before; 121 ms, max 5.8 ms after. The virtual-geometry fallback (`MeshGeometry`,
   `FullGeometry`) shares the same way. What the entry said before the fix:
   **A static mesh component copies its geometry** (`bind_mesh_asset`: `component.vertices = mesh.upload.geometry.vertices.clone()`):
   every instance of a mesh holds its own CPU copy of the vertices and indices (the GPU copy is interned), and binding
   one mesh shared by thousands of entities costs a deep copy each: 49 ms and up to 160 ms in a single
   `Assets::apply_bindings mesh`. The pump now binds one asset's dependents over several frames
   (`partial_binds`), but each bind is still a copy and every instance is memory. The proper fix is geometry held once
   (`Arc`) and the `#[gpu(content_id)]` columns reading it: a `SceneStore` derive change plus the meshes plugin
   (not done: it spans SceneDB's derive and the shadow and cluster code another agent is changing).
10. **Fixed: the first frame's input lookup read the whole project inside the tick** (`Input::scan_project`, 32 to 38 ms with the world write-locked; it held up `Game::step_scene` for 14 to 19 ms and was the "TickLoop::tick 41 to 54 ms once at start"). The session start's scan is taken by the first frame (`library::prescan`), and any other scan runs on a registered thread (`Assets::scan_in_background`; the mapping attaches the frame after it is in). `Input::Tick` max 38 ms to 0.3 ms, `Game::step_scene` max 14 to 19 ms to 2.4 to 3.8 ms. What is left in `TickLoop::tick` max (100 to 180 ms in some runs) is `TickLoop::before_systems` waiting for the scene's write lock while the frame pumps assets (`Assets::apply_bindings mesh` 20 ms to 116 ms, issue 8) or binds one material (`Texture::stage_cooked` 118 ms once at 2.7 s: a single `create_texture`, probably a driver allocation stall; creating textures on the loader threads would take it off the frame thread).
9. **First seconds of a launch still have mirror hitches**: `helio_flush_gpu_mirror` max 10 to 21 ms and `Game::step_scene`
   max 33 to 41 ms (the scatter write of rows a hydration step or a bind just dirtied), and `TickLoop::tick` 58 to 65 ms
   once, when the tick thread starts (physics and script start-up). `create_window_renderer` 1.0 s and `Helio: apply
   pipeline` 0.8 s are the renderer's own (another agent).
6. `Game::window_event` took 15 to 19 ms in earlier captures. In this round the first click costs 0.1 ms (`Game::capture_cursor` has its own scope now) and the slow events are the window's three `Resized` events at start-up (7 to 12 ms each: surface reconfigure and render-target rebuild); a size equal to the current one is ignored now. The hitch of the first click was not reproduced.
7. A recording of the busy game keeps only the last 1,000,000 events of a remote process (about 9 s of the game before the
   hot `World::get` scope was removed; the full 20 s fits now).

### Input and physics: what stalled (measured)

Physics already runs on its own registered thread (`Pulsar-Physics`, `physics_plugin/src/runtime.rs`): the game thread
sends it the journals' changes and elapsed time and never waits; it steps a fixed 1/60 s with an accumulator and the
results come back a frame later. Mouse deltas are already coalesced per tick (the input plugin sums them,
`raw::add_motion`). The stall was not a lock: it was **gameplay writing the character's rotation**. The physics
driver treats any `Transform` change that is not its own write as "someone moved this object": it fences the object
(the results of the runs already in flight are dropped as stale) and sends the capsule back to the position SceneDB
still holds. The look path (the old Blueprint wrote the rotation on every look event; now `orient_rotation_to_movement`
and `use_controller_rotation_yaw` write it every tick the view turns) did that every frame, so while the mouse moved,
walking and falling were dropped as fast as they were computed. A key alone wrote nothing, which is why it only showed
with the mouse. Measured with `installed tests character::walking_while_the_view_turns_is_not_stalled` (3 s at 60 Hz,
the view turning 2 degrees a tick while walking at 4 m/s, path length): **0.07 m before** (stalled), **11.73 m after**
(straight walk: 11.73 m), and 11.83 m with 300 synthetic mouse events a tick on top. The simulation thread's own cadence
was never the problem (and does not show it: the runs went on, their results were discarded): in the packaged game
with a key held, the mouse moving, both, and jumping, it ran 59 runs a second, longest gap 17.5 ms, longest run 0.1 to 0.2 ms
(`physics cadence` log lines, target `physics_plugin::cadence`, once a second; the first seconds of a launch show gaps up
to 1.7 s while the level loads). Fix: a rotation-only change of an object that has a character controller is no
longer a move (`turned_character` in `runtime.rs`); the capsule is symmetric about Y and the simulation never rotates it.

### Pack failures that name a missing event or native

The pack step compiles the project's classes against what the **loaded plugin libraries** registered, so a class from
one library and an event or native of another library of an older build gives "no event `input::IA_Jump.triggered` is
declared" or "no native function ... is registered". The error now ends with the libraries' load report (directory, the
libraries that did not load and why, the ones much older than the newest) and the command that fixes it
(`ui_build::runner::pack::with_library_hint`). A pack right after launch (8 s) and a pack after a full
`plugin_builder` run both succeed here; the failure needs libraries from different builds in the directory the engine
loads (the checkout's `target/release/plugins/editor` held libraries from 22 Nov that no longer load against a current engine;
`plugins/editor` held ones from 23 Nov).

### Generated games and SceneDB

SceneDB is the submodule `crates/external/scenedb`, used by path. A game's manifest is baked by
`engine_backend/build.rs` from the root `Cargo.toml` (the workspace dependency with its path made absolute, and
the whole `[patch]` table), so a game resolves the engine checkout's SceneDB, and Helio's git SceneDB source is
redirected to the same path by the baked patch. Nothing in the game tooling names a SceneDB rev. Build & Run failed
with "multiple versions of crate `pulsar_scenedb`" only because the engine binary running it was built before the
manifest change and still wrote a git SceneDB into every game; `build.rs` re-runs when the root manifest changes, so
rebuilding the engine fixed it (verified: Build & Run packs, builds and launches, the game's manifest carries the
path entries).

### Packaged game versus the editor (measured, partly)

Steady state in the packaged release game (Bistro level, editor and game sharing one GPU): frames 12 to 19 ms (the
game presents with Fifo, present queue 1), input to present 22 to 38 ms, physics 59 runs a second. Not measured: the
editor viewport's frame time (no tool reports it) and a fresh-process cold-cache comparison. What is clearly worse than
the editor is the **start**: after launch the game logs about 9200 lines in 67 s, and its first frames take seconds (one
frame 3756 ms, the next 407 ms) while materials compile (PSGC, 1750 log lines) and the level prepares; the log
is 45 percent the physics task's own phase lines (`editor_task_queue::shared: phase: ...` twice per physics run),
8 percent file-watcher lines and 19 percent PSGC. Changed: a generated game's default log filter is
`info,psgc=warn` (`RUST_LOG` still wins). Not changed and not understood: where the first frames' seconds go
(material graphs compile on a worker, so another first-frame cost: pipeline creation or texture upload), and whether
the packaged game cooks textures (BC7) or decodes them at launch. This is rendering work and needs the helio
architecture skill and the profiler tooling another agent is building; start from the first 30 s of a launch.

## Issues found and not fixed

- **A large static floor lets the capsule fall through** (`installed tests character`, a box collider of half extents
  500 x 0.5 x 500 under a capsule falling from 1.5 m: it passed the floor and ended at y = -0.3; 50 and 400 stood at
  y = 0.92; 150 rested 0.4 m low). Not understood and not fixed; the test floors use 50. The voxel project's floor is 400.

- **Mesh colliders are not simulated** (`physics_plugin/src/components/collider.rs:268`, `builder()`
  returns `None` for `ConvexHull` and `TriangleMesh`). A level's static meshes cannot collide; the Bistro
  default level uses one flat box.
- **Every plugin library has its own `engine_fs` mount state** (`engine_fs/src/virtual_fs/mounts/projects.rs:36`,
  `tree.rs`): code in a plugin library that reads a path under `plugins/<Name>/` must mount the project
  first. Three call sites do now; the structural fix is an attach hook (`_plugin_attach_engine_fs`) like the
  engine-state and settings ones, so the mounts are the host's.
- **The input plugin reports "the input mapping is not in this project"** while the Blueprint compile
  discovers asset events for `plugins/Gameplay/input/mappings/IMC_Default.inputmap`
  (`blueprint_editor_plugin::features::events::asset_events`, a WARN during every pack); the compile
  still succeeds and the events resolve at run time. Not traced.
- **The game's file watcher floods the game log** ("Applying file changes" for the asset cache folder,
  thousands of lines a second at start-up, `editor_task_queue::shared`): it pushes real lines out of the
  20000-line live log buffer. Filter with `exclude_text` for now.
- **Player start has no editor field**: `world_settings.player_start` is read by the runtime and has to be
  typed into the level file (World Settings > a registered `WorldSettingField` is the place).
- **The class compile takes 10 s or more per Blueprint** in the pack step (the world-generation class of
  the project); it blocks the build's first minute.
- **Camera possession is still the single-player view target** (`pulsar_game::view_target`,
  `camera_selection`): `ViewTargets` keyed by player, no per-output ownership, no blend, no script nodes.
  The redesign (a renderer-owned possession resource keyed by output with blend and priority rules,
  Set View Target With Blend and Get Active Camera nodes) is **not built**.
- **Viewport tools for MCP** (camera get/set, focus, screenshot as an image) are not built (`MCP.md`).
