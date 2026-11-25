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

A probe is always on (`pulsar_game::latency`, target `pulsar_game::latency`, at most four log lines a second): one
sample follows the oldest unshown input through four stamps, the OS event in the window's handler, the end of the
first tick that began after it (input drained, scripts, movement, physics, the scene written), the start of the next
frame, and that frame's `present` returning. The log line carries `input_to_tick_ms`, `tick_to_frame_ms`, `frame_ms`
and `input_to_present_ms`; `latency::summary()` is the mean and worst of the last 240. Read it from the game's
log (`logs_query` text "input latency").

Measured in the packaged release game (Bistro level, 1705 objects, 60 Hz tick, window frames about 14 ms):
input to tick 1 to 10 ms, tick to frame 5 to 9 ms, frame 14 ms, **input to present 23 to 29 ms**. So the engine's
own path is not a quarter of a second. What was: the shipped `IA_Look` had a `smooth` modifier (speed 18, a 55 ms
time constant: 95 percent of a movement took about 165 ms to arrive), an exponent 1.6 response curve and an
actuation threshold of 0.5 on a value that is already scaled to degrees, which dropped any movement slower than
about 4 pixels per frame (it looked like the camera not answering). The action now has no modifiers and a
threshold of 0.0001. The game's present queue (`desired_maximum_frame_latency`) defaulted to 2 and now to 1
(Editor Settings > Renderer > Maximum Frame Latency, restart to apply; a deeper queue adds that many frames on top of
the figure above). Test: `gameplay_defaults::the_look_action_adds_no_delay_and_drops_no_slow_movement`.
Not measured: the display's own scan-out and the monitor, play-in-editor (the probe is wired in the packaged game's window loop
only), and a before figure for the look path in milliseconds from the engine (the figures above for the old
chain are computed from its time constant, not logged).

## Issues found and not fixed

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
