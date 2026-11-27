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
  first script phase after a level is in the world, once per level (see "The game instance and level
  switching"); the editor's own edit mode spawns no roles (a level in the editor is
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

## The game instance and level switching

### What a project says

`Pulsar/project.json` is the single source (the game, the packer and the cooked content all read it;
Project Settings > **Game** edits the two keys in place and keeps every other key,
`ui_settings::utils::game`, `pulsar_content::edit_project_file`):

| Key | Absent | `""` | Value |
|---|---|---|---|
| `startup_level` | the editor's `project.default_map` (dev builds), else `scene/default.level`, `scenes/default.level`, `scenes/default_level.json` (`pulsar_content::default_level`) | **no level**: nothing loads until a script asks; no fallback, in the editor, the packer and the game | that level (a missing one falls back to the default, with a warning; the packer refuses it) |
| `game_instance` | none | none | a class folder (`classes/Main.class`); cooked content names it by class id |

The cooked settings keep `""` as `""`. The packer checks the game instance (a project class, with a
compiled module, that the trait index lists as implementing `pulsar.GameInstance`) and fails
otherwise: shipped content carries no trait index, so the game could not tell.

### The game instance

A Blueprint class implementing the **Game Instance** trait (`pulsar.GameInstance`,
`plugins/Pulsar/traits/game_instance.trait.json`; all methods have default bodies): `on_init()`,
`on_level_loaded(level: String)`, `on_shutdown()`. It is **not** a level role: World Settings does
not list it and a level switch does not end it (`pulsar_game/src/scripting/driver/game_instance.rs`).

- **Start** (`TickLoop::start_game_instance`, called by every host after the project's setup enabled
  scripting): spawned as the object `pulsar_game_instance` with its prefab components; its script
  starts at once (`begin_play`, then `on_init`) before any level is read. Native components'
  `begin_play` and the component systems (widgets, audio) start with the first tick, as every
  component's do. While no level is in the world the level roles and `LevelLoaded` wait
  (`ScriptDriver::hold_level_roles`).
- **What is its own**: its object, everything parented under it, and their components. Everything
  else in the world belongs to the level, **including objects it spawns at the root** (spawn a child
  of `Get Game Instance` to keep an object across levels; root objects spawned before the first level
  leave with that level). Its variables, timers, event subscriptions, global scripts and native
  script actors also persist.
- **`on_level_loaded`** runs in the new level's first script phase, after its objects' `begin_play`
  and the roles' hooks (possession completes a frame later). The startup level raises it too.
- **Shutdown** (`TickLoop::end_session`): the level ends first (as in a switch), then `on_shutdown`
  with the game instance's components still live, then its script's `end_play`, then the components
  end. World requests made from `on_shutdown` are dropped. Without a game instance the session ends
  as before.

### Loading and switching levels (`pulsar_game/src/tick/levels.rs`)

One flow for every host: the windowed game, headless and Play-in-Editor (`TickLoop::request_level`,
the `game::load_level` native). A new request replaces one still preparing (latest wins); loading the
current level restarts it.

1. **Prepare**, on the `level-prepare` worker, while the current level runs: read, parse, migrate,
   **validate** (`PreparedLevelFile::validate`: version, ids and parent order, every registered
   component's data decodes), scan classes, resolve roles and the render pipeline. A failure leaves
   the current level as it is; it is logged and reported as a script problem
   (`Load Level 'x' failed: ...`: the PIE Problems panel, a headless run's `problems`).
2. **Commit**, at the start of a tick: `TickLoop::end_level`: possession, `GameMode.on_game_end`,
   `GameEnded`, level scripts' `end_play` (components still live), role entities, then the level
   components' `end_play` (`Session::end_owned`), the objects (sublevels and GPU rows go with them),
   and plugins' level state (`game_hooks::LevelEndRegistration`: physics drops its simulation and
   thread; audio stops what the level's objects play, the game instance's sounds play on). Never
   `end_session`. World requests made by the leaving scripts' `end_play` are dropped. Then the new
   pipeline is selected; hosts follow it (`SceneRenderer::follow_level_pipeline`; the same pipeline is
   not rebuilt; the old one draws until a new one is built).
3. **Hydrate**, `HYDRATION_BUDGET` (4 ms) a tick. While the level is partial **nothing of gameplay
   ticks** (no system, component, script or timer; input is still flushed to the hub, rendering goes
   on). The game instance is paused too. Then the level's roles and `world_settings` become the
   session's (gravity before any physics component begins), and the next tick runs. Physics starts a
   fresh simulation for the level, so it again waits for mesh collider geometry
   (`holds_for_geometry`).

Headless runs (and tests) load blocking: the whole switch happens in the tick that finds the request,
so a run is the same on any machine. A failure of hydration after the commit (what validation cannot
see) is reported and the game runs with what loaded.

**No level.** Before the first level (or when none loads) the renderer runs the fallback pipeline a
plugin offers (`pipelines::FallbackPipeline`; `pulsar.default_pipelines` offers
`Deferred.rendergraph`, which has the widget nodes), so the game instance's widgets draw. The packer
cooks it when the project has a game instance or starts with no level. A loaded level that names no
pipeline still shows the "No render pipeline assigned" screen.

### Script nodes (every Blueprint; `scripting/game_natives.rs`, palette category Game)

| Node | Native | Pins |
|---|---|---|
| Load Level | `game::load_level` | `level` (string; the pin editor is the `.level` picker, `pin_type.level = LevelPath`) |
| Set Input Mode | `game::set_input_mode` | `mode` (`Game` or `UI`; a dropdown, `pin_type.mode = InputModeName`) |
| Quit Game | `game::quit` | |
| Get Game Instance | `game::instance` | result: the game instance's entity (null without one) |

The first three are queued like `world::spawn` and applied by the tick loop after the script phase.
Blueprint pin editors honour any native's `pin_type.<pin>` attribute (the name of a reflected type
with a property editor).

**Input mode UI** (`game_control::InputMode`, kept across levels): windowed, the cursor is released
and a click does not capture it; the free camera and the late look stand down; the input runtime
reads no device (`GameplayInputSuppressed`: held actions complete, a key held across the menu counts
again when pressed again). Raw key and mouse events still reach scripts (a pause menu's Escape), and
widgets get the pointer. Back to `Game` captures the cursor at once if the window has focus (the
start of a session is `Game`, captured on the first click, as before). Esc frees the cursor in every
mode. Headless ignores the cursor; the input suppression applies. Play-in-Editor: the Game tab never
grabs the OS cursor; UI mode suppresses gameplay input and the free camera the same way.

**Quit Game**: windowed closes the window and ends the session; a headless run stops
(`HeadlessReport::quit`); Play-in-Editor stops Play (PIE ABI v6 `control::QUIT_REQUESTED`).

**Play-in-Editor** plays the level the editor has open, not the default level: the game instance
starts first, and a Load Level in its `on_init` replaces the open level as it would the default one.
Load Level of the open level reads the Play snapshot (`target/pie/play.level`, unsaved edits
included; the ABI passes the open level's name). Stop restores the editor's world from its pre-Play
snapshot, whatever levels were loaded. Saving is refused while Play runs: the world is the game's.
The fast path reuses a built game library only if this editor build made it
(`<library>.engine-stamp`: the PIE ABI and the editor executable) as well as no source being newer.

How others do it: Unreal's `UGameInstance` outlives every `UWorld`; `OpenLevel` tears the world down
and loads the next behind a loading screen; Unity keeps `DontDestroyOnLoad` objects across
`SceneManager.LoadSceneAsync`; `FInputModeUIOnly` / `FInputModeGameOnly` are the input modes.

Tests: `plugins/installed/tests/game_instance.rs` (packaged and headless: order, no level before a
widget click, two switches and a reload, state surviving, nothing left behind, gravity per level, a
failed load, Quit, `on_init`'s Load Level over the default; hydration across ticks with gameplay
held; Play-in-Editor switches and Stop restoring the world).

### Not done

- No loading screen while a level hydrates; the game instance cannot tick during hydration (its
  widgets stay on screen, unchanging).
- No seamless/additive travel: sublevels are `SubLevelDisplayComponent`; there is no streaming of a
  second level beside the current one.
- The input mode's `Game` state has no "capture without a click at start" option.

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

### Collision with a level's geometry

A `ColliderComponent` of shape `TriangleMesh` (or `ConvexHull`) with an empty `mesh` is made from the
geometry its object shows **and that of every object under it** (`physics_plugin/src/components/collider_geometry.rs`),
so one collider on a level folder makes everything in it solid. It reads the engine's content geometry
(`content_triangles`, which meshes register), places each object's triangles through the hierarchy
(`world_matrix_of`), and builds the shape on the physics thread (the Bistro: 2.8 million triangles, 0.8 s).

- The simulation does not start while such a collider's level is still loading assets
  (`holds_for_geometry`, capped at 30 s): the packaged game streams meshes in after play begins.
- Not handled: geometry that appears later (a sublevel, a streamed or spawned mesh) does not rebuild the
  collider until the collider itself changes; a mesh named by path in `mesh` is not simulated.
- Every triangle collides, foliage included.

### Input latency

A probe is always on (`pulsar_game::latency`; at most four debug-level log lines a second, target `pulsar_game::latency`, shown with `RUST_LOG=pulsar_game::latency=debug`): one
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
- **The frame reads the world after the previous frame's present, and records before it acquires.** Round 2 acquired the
  surface first (the wait for the display) and then read the world; the frame was then recorded while the GPU idled, and the
  period was the CPU's time plus the GPU's (issue 14). `GameWindow::render` now steps the scene and takes the camera
  (`Game::step_scene`, `Game::camera_for_frame`) at the start of the frame, records and submits into its own
  texture, and only then acquires (the display wait) and copies into the image. Input to present with vsync off improved
  (13.2 to 9.8 ms mean); with vsync on, windowed, the mean is 10.4 before and 12.0 ms after (p95 17.4 to 14.6): the
  camera is as old as the wait for the display again, which is the price of the CPU running ahead.
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
   binds the whole table as before. **The unassigned-texture hazard is closed:** a graph material's generated WGSL indexes `scene_textures[material.tex_*]` without a guard, and an unassigned field is `u32::MAX`, which the shader's bounds handling clamps to the last element of the array, unbound in a partially bound table. Slot 0 of the scene's texture table is reserved now: the host creates the store with `TextureStore::with_reserved_slots(capacity, helio_core::TEXTURE_TABLE_RESERVED_SLOTS)` (SceneDB hands no texture slot 0 out and never recycles it, `helio_bridge`), the materials table binds its white fallback view there and refuses a texture in a reserved slot (`MaterialTextureTable::set`), and a graph material's caller maps each unassigned field to it (`material_graph_texture_slot` in `material_row.wgsl`, applied where `pulsar_material_graph` is called, `GpuMaterial::FALLBACK_TEXTURE_SLOT`). Tests: the lowering (`an_unassigned_texture_field_reads_the_white_slot`), the Rust and WGSL constants agreeing, the store (`reserved_slots_are_never_handed_out_or_recycled`).
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

11. **The "walking regression" was a change of scenario, not of code** (six packaged games, two scripted runs each, one
    machine, project copy `tb_perf_project_r3`, `drive.ps1`: 2 s idle, walk and turn, jumps, strafe, flicks). The builds
    from before the late-look commits and the one after differ in what the game does: the early ones (base, a) log
    `No roles are set for this level; nothing was instantiated`, so the walk drives a free camera; the later ones (r2b,
    c, no-partial-table, and every build since) log `Player Character spawned class=PlayerCharacter` and the walk is the
    character's, through physics, from a different spawn view (a street instead of a terrace; screenshots at idle, and
    a build's exe, not its Content, decides it: an exe of one with the Content of the other shows its own view).
    Frame period over the 20 s run: 6.06 to 6.11 ms (119 to 141 fps 1% low) without a character; 6.40 to 6.61 (93 to 97)
    with one, in all four builds that have it, so no commit between them moved it. The GPU frame follows the view, not
    the code: 3.4 to 3.5 ms (Atmosphere 0.70 flat, ObjectBatch and VgBvh 0.02) without the character against 4.6 to 4.8
    (Atmosphere 1.3 at idle and up to 2.3 looking at the sky, ObjectBatch 0.2, VgBvh 0.03 to 0.4) with it, already in the
    first two idle seconds, before any walking; `partial_table` on or off made no difference. The numbers the 6.9 ms /
    92 fps walking figures were compared to were taken in a game with no character; the character's walk and turn is
    6.7 to 7.0 ms (91 to 93 fps 1% low) in every build that has it. A comparison must run the same level role setup.
12. **Atmosphere (GPU 1.2 to 1.7 ms of a 4.8 ms frame) in the character scenario.** Skipping parts of the pass on the
    game (idle, same camera): the cloud work (planet weather slice and shadow map) 0.3 ms, the sky-view, aerial and
    irradiance kernels 0.1 ms, the params-only LUTs 0.18 ms amortised, the rest (a dozen dispatches and their
    barriers, about 0.8 ms) was not separated; the LUT rebuild runs every frame while the level streams in (the
    atmosphere buffer's content generation moves with every mirror flush) and never after. Refreshing the cloud shadow
    map a quarter of its texels a frame (2x2 interleave, a whole refresh when a cascade's lattice or the sun moved) lowered
    Atmosphere 1.66 to 1.69 to 1.61 to 1.66 ms, inside the run to run spread, so it was not kept.
    **Fixed (round 4): Atmosphere 1.8 ms to 0.23 ms.** Per-dispatch GPU scopes (`sky/feature/src/steps.rs`: while the
    profiler records GPU time each dispatch is its own compute pass inside a named scope; unprofiled they share one pass)
    showed the unexplained 0.8 ms was not barriers: `Atmosphere: resolve`, a single-thread kernel, took 0.56 ms (it walked
    the atmosphere row table one memory latency per row, and the table holds a row for every slot the component has used),
    and `Clouds: planet weather` 0.74 ms was the whole planet weather map rebuilt on two thirds of frames, with the
    multi-scattering LUT (0.19 ms) beside it. Both ran because the row join's output `content_generation` moves whenever any
    transform moves (the join reruns when its `transforms` input changes, and the character moves), which the CPU-side
    "rebuild when the rows changed" key took for a change of the atmosphere. Now the GPU plans the frame
    (`SkyPlan`, `sky_layers.wgsl`): `resolve` is a 256-thread workgroup that finds the first enabled row together, hashes
    the words of that row, compares with the last frame's hash and writes indirect dispatch arguments (zero groups unless the
    row changed) for the transmittance, multi-scattering and sky-table kernels and for the weather bake, which reads
    `rebuilt` to take its whole map or this frame's slice. Nothing is read back; the CPU-side generation key, the `params_only`
    flag and `bake_whole` are gone, and so is the `HELIO_ATMOSPHERE_TRACE` diagnostic. What is left of Atmosphere: cloud
    shadow map 0.077 ms, aerial perspective 0.046, irradiance 0.031, the rest under 0.02. The joined buffer's coarse
    generation is still wrong for every other consumer of a `RowJoin` (open, issue 18).
13. **The sun's cascades were redrawn on two thirds of frames, a still camera included (Shadow 0.6 ms GPU).** Not the
    camera noise the first fix assumed: the cascade matrices differed in their last bits from frame to frame, and the
    change detection hashes the bits. The texel snap rounded the centroid in a view centred on the centroid (a value that is
    zero by construction, so it snapped nothing) and the light's basis came from `look_at` with the camera's position in it.
    `shadow_matrices.wgsl` now takes the axes from the light's direction alone and moves the centre on a fixed lattice
    (16 cells across the cascade, a whole number of texels each; the radius carries the half cell diagonal, 1.097). A first
    attempt that only coarsened the lattice (kept centred on itself) changed nothing: Shadow 0.56 to 0.57 ms, reverted.
    Shadow 0.62 to 0.17 / 0.28 ms, GPU frame 3.33 to 2.98 / 3.00 ms. Also: the static layer's pass (clear draw and per-face
    draws, 1.1 ms of GPU time when it ran with nothing to draw) opens only when a serviced face rebuilds its static layer or
    the readback shows its pending static bit (`pending_bits`); alone that changed nothing (3.33 to 3.33 ms) because the
    cascades really were dirty, and it is kept for the visits a walking character causes.
14. **The frame was recorded after the swapchain acquire, so the period was the CPU's time plus the GPU's.** With a present
    queue of one, `acquire` waits for the GPU to finish the frame before, and everything after it was recorded while the GPU
    idled (CPU frame 3.0 ms, GPU 3.0 ms, period 6.2 ms). `GameWindow::render` now records and submits into a texture of its
    own (`SceneTarget`), acquires afterwards (the display wait) and copies the scene into the image (`Game::copy_to_surface`,
    0.19 ms CPU at 1280x720); the world is sampled at the start of the frame, as before. Period 6.2 to 3.3 ms (vsync off),
    input to present (profile_latency), vsync off: 13.2 ms before the round, 11.3 after the GPU work, 9.8 after this; vsync
    on, windowed: 10.4 before and 12.0 after (p95 17.4 and 14.6), the mean 1.5 ms worse (the world is read before the display
    wait again), the tail better.
15. **`window.fullscreen` was never read.** `pulsar_content` has the setting and nothing used it, so every "fullscreen" run was a
    1280x720 window. `WindowDescriptor::fullscreen` is filled from it and `open_window` asks for borderless fullscreen. At
    the display's 3440x1440 the GPU frame is 2.4 times the windowed one (see the tables), and that is the real case.
16. **The editor viewport's first pipeline of a session blocked its render thread for 12.3 s** (`Helio: rebuild graph`, a
    configuration rebuild inside `render()`; `Helio::Renderer::render` max 12340 ms, the UI thread's redraw stalled 3156 ms).
    A rebuild builds off the frame thread like an assigned pipeline now (`PipelineBuild` has a kind; `begin_rebuild`;
    `Renderer::set_blocking_builds`, default true, off for `SceneRenderer` in background mode) and the editor viewport uses
    `PipelineBuildMode::Background`. After: the render thread's longest frame 47 ms, the UI thread's longest redraw
    258 ms, the viewport shows the status screen "Building the render pipeline" until the build ends. The build itself
    still takes 11 to 13 s there against under a second in the game: the editor's device is shared with the UI and the
    compile throughput is the open question (issue 19). Opening a level also starts a second renderer's build and a
    configuration rebuild (three builds, about 38 s of worker time, none on a frame).
17. **What is left in the frame, vsync off, windowed (after):** GPU 2.5 ms (DeferredLight+AtmosphereComposite 0.37, Shadow
    0.28, ObjectBatch 0.24, Atmosphere 0.23, VgBvh 0.22, GBuffer 0.15, LightCull 0.13) and CPU about 3 ms in a frame
    (`encoder.finish` 2.6 ms over four worker threads with the frame thread waiting 0.94 ms of it, `queue.submit` 0.44,
    scene derivations 0.18, publish host parts 0.12, copy 0.19, present 0.26). Fullscreen (after): GPU 6.1 ms,
    DeferredLight+AtmosphereComposite 2.1 (35 percent: it scales with the pixels, a fused chain that is timed as one
    span; the shadow filter samples and the sky composite are not separated), GBuffer 0.54, HiZBuild 0.41, Shadow 0.31,
    ObjectBatch 0.30, VgBvh 0.27, Atmosphere 0.26. At 3440x1440 the game is GPU bound at 145 fps against a 165 Hz display.
18. **Open: a `RowJoin` reruns, and moves its output's `content_generation`, whenever any input buffer changed**
    (`helio-scene-join/src/row_join.rs`, the `signature` of owners, generations, hidden, `transforms`...): an atmosphere row
    placed by its owner's transform is rewritten whenever any transform in the world moves. Passes that key work on that
    generation (the sky did) rebuild for nothing. Consumers found: the sky (fixed, issue 12); the lights' shadow pass (handles
    it: it re-solves only); the light sampler, HLFS and light cull rebuild their grids and lists (LightSamples 0.06 ms,
    LightCull 0.13 ms of GPU, and their finish costs) on every frame anything moves; VgBvh refits its six passes every frame
    (0.2 to 0.27 ms of GPU, 0.38 ms of finish thread) because the virtual-geometry instance rows' generation moves with a
    walking character, which is not in the BVH. The CPU sees one counter per buffer, so the honest fix is in the join or in
    SceneDB: a per-table change bit computed on the GPU and used as the sky now uses its plan (indirect arguments of zero),
    or the mirror's dirty-row journal handed to the join. Not done: it changes the join's contract for every consumer.
19. **Pipeline build throughput in the editor: answered by issue 22** (11 to 13 s for the Deferred pipeline against under a second in the game,
    and the voxel pipelines' 14 to 17 s), a one-off hitch of 150 to 650 ms in about one run in two (the frame has no scope
    longer than 30 ms inside it; before and after), the deferred lighting cost at native resolution, and `encoder.finish`
    (2.6 ms a frame in four threads).
20. **Round 4, second pass (deferred lighting, the stall, `encoder.finish`): measured, nothing kept for the GPU, one mitigation
    for the stall.** Fullscreen 3440x1440, vsync off, GPU frame mean ms (two runs of the unchanged build: 5.76 and 6.02):
    - *Deferred lighting, split.* `DeferredLight+AtmosphereComposite` is a fused chain timed as one span. Run with the chain
      split (a temporary switch in the executor, removed): `DeferredLight` 2.00 ms, the composite under the top twelve
      passes. The same pass with the composite's fragment returning at once measured 1.0 ms less, but the whole frame fell
      2.1 ms and earlier passes too (GBuffer 0.43 to 0.12): a lighter frame lets the GPU clock up, so a pass's time moves with
      the frame's load. Skipping the reflection draw (`fs_reflection`, a second draw in the same pass): 1.94 ms (nothing).
      `ENABLE_LIGHTING` off: the GPU frame fell to 3.61 ms (direct lighting and its shadows are about 1.8 of the 1.96 ms).
      Shadow factor forced to 1: the chain 1.31 ms (shadow sampling about 0.65 ms; the other 1.1 ms is the per-light loop, BSDF
      and the G-buffer reads, not separated).
    - *Candidates, each two runs against the unchanged build, reverted:* the cloud taps skipped for geometry nearer than the
      layer's floor (`clouds_in_front`, exact): GPU 5.79 / 5.91 ms and the chain 1.94, against 5.76 / 6.02 and 1.96; the glass
      transmittance lookup skipped when ObjectBatch counts no translucent shadow caster (`light_transmittance`, exact):
      5.90 / 6.12 and 2.00 / 2.13. The GPU loop stops here (two candidates in a row inside the spread). What is left is the
      light loop's ALU, about 17000 lane cycles a pixel at this resolution; the next design is clustered or tiled shading by
      light class (the sun separately, local lights by tile with a shadow-face budget), which needs a GPU capture's
      per-instruction profile.
    - *The 150 to 650 ms stall* (once in 1 to 4 runs of 20 s, before and after): found with scopes. The hitch frame's own children
      total 5 ms and the missing time is after `Game::present`: `Game::latency_publish`, the input-latency probe's
      `frame_presented`, 276 ms in the frame caught (new scopes `Game::latency_look`, `latency_submit`, `latency_log`;
      `DrivenFrame: task tick`, `publish details`). The profiler's submit is a lock-free push, so the suspect is the `tracing::info!`
      line it wrote up to four times a second on the frame thread (a log layer's lock, held by a flood from another thread).
      It is a debug-level log now (`RUST_LOG=pulsar_game::latency=debug`). Six further runs showed no hitch, which does not prove
      an intermittent fault: the scopes stay, and a recording with a hitch names the scope.
    - *`encoder.finish` per pass* (`HELIO_FINISH_BREAKDOWN=1`, the existing diagnostic; windowed, vsync off; ms a frame on the
      finish threads): VgBvh 0.38 (a refit of six compute passes every frame, issue 18), Lens 0.15, MeshHitSurfaces gi_probe
      0.14, ObjectBatch 0.14, Atmosphere 0.11, Shadow 0.11, LensFlare 0.10, PostProcess 0.09, GBuffer 0.07; about 2.2 in all.
      Binding 64 views of the material table instead of the live prefix moved the sum 2.18 to 1.95 and the frame thread's wait
      0.94 to 0.69, so the table's views are a tenth of it; the rest is per pass and per command validation (about 30 us a pass).
      Fewer, fuller passes (the BVH's six, the lens and post chain's) are the lever; not done.
    - *Checks run:* Helio's own tests from its own workspace (`cd crates/renderer/helio && cargo test -p helio --release`, its own
      `target`): 23 unit tests, `pipeline_status` 9, `status_screen` 4, `scene_db_projection` 1; Pulsar's `installed_plugins`
      `default_pipelines` 8, `pipeline_build_async` 4, `plugin_pass_lists` 1 (the pass lists are unchanged) and
      `render_acceptance` 38 passed with `--test-threads=1` (including `the_sky_is_the_levels_atmosphere`, which runs the
      sky's GPU plan); in parallel 18 of them fail on a shared GPU. A first run found a real regression of this round (the editor
      renderer always built in the background, and the harness reads its first frame): the mode is the host's now
      (`HelioRenderer::set_pipeline_build`, blocking by default, background for the editor's `GpuRenderer`).
    - *Measurement hazards:* the editor restores the level tab it had open, and a level viewport at 146 fps takes 70 percent of the GPU
      (delete `.pulsar/layout.json` before a capture); another agent's GPU tests hold `D:\perf_gpu.lock` for minutes and break the
      voxel build in between (the capture scripts wait for the lock and retry a build that fails in voxel).
21. **Editor open: where 12 to 15 s go, and what changed** (the editor viewport, `default.level` of the Bistro project, from the
    `editor_open_file` call). *(a)* The renderer exists within a second of the open; the time is the render pipeline build, 11.7 to
    12.6 s: the nodes build one after another on one worker, each creating its GPU pipelines as its passes are constructed
    (`helio_graph_asset` logs a node that takes 150 ms or more, target `helio::pipeline`): virtual_geometry (12 passes) 2.5 to
    3.4 s, hlfs 4.0, lens 1.4 to 1.8, water_sim 1.2 to 1.7, foliage_place 0.8 to 1.2, atmosphere 0.9 to 1.3, deferred_light 0.7 to
    1.0, the other twenty-odd nodes 0.2 to 0.6 each. The game builds the same pipeline in 1.5 s (not the instance flags, empty in
    both; not the voxel compiles, which start after the first build). A level also asks for a second build right after the
    first (`Rebuild render pipeline`, 12.6 s): `prepare_frame` sets the TSR preset from the scene's content every frame
    (`set_tsr_quality`), and the content that decides it arrives after the first build started, so a configuration the build did
    not have rebuilds the whole pipeline to change a preset and a render scale. Not changed (it is the owner's choice of quality):
    hold the first assignment until the content that picks the preset is known, or make the TSR switch local to its pass.
    *(b)* Done: the pools are sized to the machine (`editor_task_queue::default_worker_counts`: dedicated a quarter of the cores,
    2 to 8, short an eighth, 3 to 6; it was 2 and 3). A long task holds its worker for its whole run (a 12 s pipeline build, a voxel
    bundle), and the bundles of the voxel world queued behind it: their compiles went from 14.3, 16.7 and 3.1 s (waited
    0.0 s, built in) to 3.2, 6.1 and 1.8 s in the same editor, opened the same way. *(c)* Not done here (it is the voxel
    feature's design): the compiles start when the pass first sees the volume, after the pipeline is installed (the first
    build's end); started when the world is built (the component has the device through `render_runtimes`) they overlap the 12 s
    build and the first chunk waits for the pipeline only. *(d)* Done: `helio_core::compile::SideBySide` and `Slot` (the voxel
    feature's `Kernels` and `Kernel`, moved), on `helio_tasks::parallel` and `parallelism`, new methods of `TaskHost` (the
    default is scoped std threads; the editor's host runs the lanes on its rayon pool, `EditorTaskHost::parallel`). The voxel
    feature's `chunks/kernels.rs` is now a re-export of the two names under their old names and its two tests moved to
    helio-core (`side_by_side_tests`, passing in Helio's own workspace). *(e)* The UI thread's 3.1 s and 0.75 s `NavSidebar`
    render stalls: the sidebar's tab list reads each open panel's decoration (`panel_file_path`, `tab_icon`, `tab_unsaved`
    of the level editor, each a `shared_state.read()`), and the level editor's `gpu_engine` mutex is what the render thread
    held for the 12 s blocking build: the stalls coincided with it and are gone with it (editor recording from before the open:
    longest main-thread scope 8 ms, longest Helio Runtimes frame 5 ms; before: 3156 and 12343 ms; first background change:
    258 and 47). The lock itself was not isolated (the sidebar's render has no scopes inside); a `try_read` with the last
    decoration would remove the class and was not needed once the build left the frame. *Time to the first voxel chunk* was not
    measured here (the Earth-sized project is the voxel agent's); the parts that moved are the pools and the compile helper above:
    on this project the voxel bundles that took 14 to 17 s each now take 2 to 6 s, concurrently with the 12 s build.

22. **Editor open (round 5): why the editor's pipeline build took 12 s and the game's 1.5 s, and what changed** (the Earth voxel
    project `D:\tb_voxel_proj` and the Bistro project; release editor, RTX 4090, 32 threads; seconds from "Opening project" unless
    said; two runs each, the original editor and this one interleaved).
    *The cause of 8x (issue 19 and 21 (a))*: the editor ran on **Direct3D 12**, the game on **Vulkan**. `renderer.toml` of this user holds
    `backend_preference = "dx12"` (the editor's compatibility notice offers it: `known_issues.json` says Vulkan may lose render-target
    writes intermittently and not on D3D12), the game reads no editor settings and takes `auto`, which is Vulkan. On D3D12 every pipeline
    goes through DXC and the driver on the calling thread, one after another, and wgpu has no pipeline cache there
    (`Features::PIPELINE_CACHE` is Vulkan's, `device.features()` of the editor's device lacks it: the "driver pipeline cache" persistence
    and the `PULSAR_PIPELINE_CACHE` file never applied in the editor; a second build in the same process cost the same as the first,
    node for node). Not the instance flags, shader hot reload, a debug helio or the validation layers. The engine now logs the adapter
    and backend (`Engine device: ... through Dx12 ..., pipeline cache not available on this backend`) and, per node of a build, how many
    shader modules and pipelines it created and the time in each (`helio::pipeline: node ... built N pass(es) in X ms: M shader module(s),
    P pipeline(s) T ms`, and `graph ...: N node(s) built in X ms, locked in Y ms; slowest: ...`).
    *What changed*: (1) **a build whose inputs changed is abandoned, not finished** (`Renderer::supersede_build`, `IntoGraph::duplicate`,
    a cancel token the build loop reads per node): the TSR preset the scene's content picks arrives about 50 ms after the first build
    starts, and the old behaviour finished the whole stale 11 s build and then built again (13 s). Now the stale build stops at its next
    node and the new one starts at once; no configuration change waits behind a build any more. (2) **Pipelines compile side by side**
    (`helio_core::shader::create_*_pipeline_deferred`, `batched`, `Deferred<T>`): inside a build the descriptor is queued and answers a
    handle that derefs to the pipeline; three workers compile while the nodes are still being named, every lane of the host's compute
    pool (`helio_tasks::parallel`, up to 24) takes what is left when the build ends, and `batched` returns when all are built (a rejected
    pipeline is the build's failure with the backend's reason; an abandoned build compiles nothing more). Outside a build it compiles on
    the spot. Migrated: the nodes that held the build (virtual geometry's cull, lens, water, atmosphere and clouds, foliage, deferred light,
    shadows, fog, GI, object batch, lights' cull/sampler/HLFS, the VG raster/shade/BVH...); the other `create_*_pipeline` sites still
    compile in line, and a pass that uses a pipeline while it builds (`get_bind_group_layout` in its constructor) makes the build wait for
    it (`pipeline X was used at build time and waited N ms` names it: the lights' remapped tiles pass is one, and stays immediate).
    (3) **The voxel world's pipelines compile while the graph builds** (`helio_core::BuildHints`: the renderer's published payloads and
    whether this is its first build, as a build setting; `ChunkPass::precompile` adds the volumes at node construction, so the draw,
    generation and ray bundles start in the first half second instead of when the first frame meets the volume). (4) The UI thread no
    longer waits on the level editor's state for a tab: `LevelEditorPanel::tab_decoration` reads it with `try_read` and draws the last
    copy when a writer holds it (the sidebar, the dock and the title read file, icon and unsaved through it), the sidebar's render has
    scopes (`NavSidebar::sidebar_tabs`, "a panel's tab decoration") so a stall names its lock, and the Panel methods name theirs.
    (5) `PULSAR_PIPELINE_CACHE` defaults to the per-user cache directory when the editor has no build directory (Vulkan only).
    (6) **Viewport frame policy** (see `.agents/RENDER_RUNTIMES.md`, "Frame policy"): the table below.
    *Measured, Earth voxel project* (original editor, then this one; two or three runs): first render pipeline installed 14.5 and 15.7 s
    then 4.5, 6.5, 7.7, 7.8, 8.2 s (the build 11.2 to 11.4 s then 3.3 to 4.7 s); the rebuild the TSR preset caused finished at 27.5 and
    29.3 s and no longer exists; first voxel chunk drawn 21.4 and 22.4 s then 10.8, 11.3, 11.5 s (12.7 and 16.1 s in two earlier
    builds of this work, before the voxel compiles overlapped); settled view 23.4 and 24.9 s then 13.3 to 13.5 s. Bistro project
    (`default.level` opened over MCP, seconds after the call): pipeline installed 11.8 s then 4.3 s, rebuild 14 s then none, first chunk
    30.6 and 30.8 s then 20.1 and 20.2 s (that project's startup, not the open, holds the first 10 s of those). Longest `Slow UI work`
    in the open: 206 to 414 ms before, 179 to 333 ms after, all of them the first `WindowEvent::Resized` and the first
    `effects (observers, subscriptions, notifications)`, none during the build; the old 3.1 s `NavSidebar` stall is not reproduced on
    either (it needed the blocking build). *Second open*: unchanged by design on D3D12 (no pipeline cache exists there; the driver's own
    cache did not shorten a second build in the same process: 5.0 s and 4.5 s for the same graph); on Vulkan the persistent cache
    now saves under the user cache directory. *What is left*: 3.3 to 4.7 s of build is node construction in line (about 3 s; the
    biggest are virtual geometry 0.5 s, GI 0.5, sprites 0.2) plus the tail of the compile; the voxel generation bundle's longest kernel
    (`feature_block_candidates_main`, 6 s of DXC on its own) now bounds the first chunk: splitting it is the voxel feature's change.
    The Vulkan intermittent render-target-write loss that makes this user's editor run on D3D12 applies to the game too, which runs on
    Vulkan. A window restored from minimized, driven by `ShowWindow` from another process, stopped consuming the viewport's frames after
    a few frames in the original editor as in this one (frames 0 GPU 0 percent in both); not reproduced by hand, left open.
    *Viewport frame policy, measured* (the Earth project, level tab open as the project restores it; GPU utilisation is nvidia-smi's over
    10 s, frames are the runtime's published frames a second, the window state is the engine's own report): original editor: focused
    146 fps 54 percent GPU, visible but unfocused 146 fps 54.5 percent, hidden tab 0.6 percent, minimized 0.7 percent (the OS held the
    frames). This editor: focused and idle 15 fps (`Ambient`: only the foliage wind moves), 32 percent busy (the profiler: 21.4 ms of GPU
    a frame at 15 fps against 3.7 ms at 146 fps, the GPU clocks down, so busy time falls by 40 percent where the frame rate falls by
    90), visible but unfocused 9 fps 24 percent, hidden tab 0 frames 0.3 percent, minimized 0 frames 1.1 percent, a camera move or an edit
    back at the display's rate (60 to 160 fps while it lasts, then ambient again 0.4 s later). `ambient_fps = 0` keeps the old free-run.
    Found on the way: an overlay that was hidden withdrew its view payloads every UI frame and each withdrawal counted as a change, so
    the viewport never went ambient (`SceneRenderer::set_view_payload` now says whether anything changed).
    *The game* (Build & Run of the Bistro project, Vulkan, the build above): the Deferred graph's 68 nodes took 6.5 s on a cold driver
    cache and 3.6 s with the cache file of the first run; the 1.5 s of the earlier measurement was not reproduced (that run held the
    cache file and a graph that built each node once). While measuring it, `driver_cache_for_device` keyed the cache by the address of
    the `&Device` it was asked with, so every clone of the handle (each compile worker's, the deferred jobs') loaded the 17 MB cache file
    again: it is keyed by the device's identity now (fixed after this game run; the game was not measured again). The game starts, draws
    and plays on this build.
    *Not done*: the parallel `render_acceptance` run on a shared GPU (18 failing before) was not run against the base build here; the
    Vulkan build of `helio` and the plugin passes' own GPU tests were not re-run beyond `helio-core`'s deferred tests (6 pass on this GPU)
    and `engine_backend`'s `pacing` tests (8 pass); the other `create_*_pipeline` sites (about 120) still compile in line.

### Round 5: the built game at 3440x1440 (2026-11-27)

Reported: about 85 fps on an RTX 4090 at 3440x1440. Measured with `PULSAR_FRAME_STATS=2` (`pulsar_game::frame_stats`: every
2 s the frame rate, each CPU stage of the game window's frame as mean/worst, the render graph's CPU and GPU pass times, and every
fifth report wgpu's allocator report by allocation name), release game, 165 Hz display, Fifo, frame latency 1, steady state after
the level streamed in. `coin_town` was loaded as the startup level from a scratch content folder linking the project's loose files;
the menu level (`coin_town_menu`) is the same street with nothing moving.

| | menu, before | coin_town, before | coin_town, after |
|---|---|---|---|
| fps | 139 to 146 | 101 to 106 | 111.8 |
| frame period ms | 6.9 to 7.2 | 9.4 to 9.9 | 8.95 |
| GPU frame ms | 6.8 to 7.1 | 8.4 to 8.8 | 7.95 |
| CPU: record and submit ms | 3.0 to 4.2 | 4.0 to 4.5 | 3.06 |
| CPU: render graph passes ms | | 0.81 to 0.90 | 0.58 |
| CPU: acquire (waiting for the GPU) ms | 1.7 to 3.2 | 3.5 to 3.8 | 4.85 |
| CPU: copy and present ms | 0.7 to 1.0 | 1.2 to 1.3 | 0.77 |
| ObjectBatch GPU ms | 0.14 | 0.49 (full rebuild 300 of 300 frames) | under 0.07 (moved-slot path) |
| VgBvh GPU ms | none | 0.29 (rebuild every frame) | under 0.07 |
| VRAM allocated (reserved) | | 10.85 GB (12.35) | 4.65 GB (5.18) |
| of which textures | | 8.26 GB, 565 textures | 2.07 GB |

The first 30 to 40 s after the level starts run at 45 to 78 fps while it streams in (`record_submit` 11 to 22 ms: asset binding and
uploads); that and the overlay's 1% low (82) are what "about 85" was. The game is GPU bound at this size throughout.

Fixed:
- *No texture compression in the game.* `request_engine_device` (the factory every engine device comes from) now asks for
  `TEXTURE_COMPRESSION_BC` where the adapter has it; only the editor's window asked before (`engine/startup.rs`, removed), so the game
  cooked and held every texture as RGBA8 (`CookFormat::best_for`). Four times the VRAM and the sampling bandwidth. See
  `.agents/MEMORY.md`. The first run after this re-cooks the textures as BC7 (cached in the derived cache after that).
- *Moving objects re-derived the virtual-geometry instances every frame.* `VirtualGeometryInstances` keyed its rerun on the object
  rows' content, which moves with every transform, and every rerun moved its outputs' generations: the object batch saw the
  classic-suppressed buffer change and re-sorted the whole scene (a full rebuild every frame), and the VG BVH saw new instances and
  rebuilt instead of refitting. It reads only the rows' structure (mesh slot, generation, flags), so it keys on the scene join's
  structure marker now; the one transform-dependent bit, whether cone culling is valid (uniform scale, no mirroring), moved to the
  VG cull, which loads the transform every frame anyway (`vg_cull.wgsl`, `cone_cull_allowed`).

What the 7.95 ms GPU frame is now (coin_town, ms): DeferredLight+AtmosphereComposite 1.69; the GI chain about 2.9 at the project's
`gi.quality = pixels_16` (Denoise 0.78 with reconstruct 0.31, temporal 0.24, spatial 0.19; GIGatherShade 0.39; GIGather 0.31 with near
field 0.26 and spatial 0.24; MeshRays gi_pixels 0.19; GILight 0.18; HistoryPixels 0.16; MeshRays gi 0.16; LightSamples 0.13; GIResolve
0.11; the rest under 0.1 each); GBuffer 0.55; HiZBuild 0.47; Atmosphere 0.32; PostProcess 0.27; Shadow Matrix residency solve 0.21;
portal occurrences 0.26 in two passes; Lens 0.13. Open:
- *HiZBuild 0.47 ms*, rebuilt every frame anything moves: it copies the full-resolution depth into a buffer and back into the
  pyramid's mip 0, then runs one dispatch per mip level. A single-pass downsampler that reads the depth texture directly (as AMD's
  SPD does) removes both copies and the per-mip barriers; it needs the translated-shader concern in the pass's header checked.
- *Shadow Matrix residency solve 0.21 ms* every frame something moves.
- *The detail budget does not hold* (`texture_detail`): textures bound before the total crossed the budget keep their full size.
- The CPU frame (about 4.5 ms of record, submit, copy and present) is under the GPU's at this size; at 1280x720 the game sits at
  the 165 Hz refresh (GPU 3.5 ms, CPU about 3.5 ms).

### Round 4: measurement and the profile it began from

Rules: the existing recorder (`profile_start`, `profile_gpu`, `profile_query_sql`) on the packaged game, one fixed
scenario (project copy `tb_perf_project_r3`, the character spawned, `drive.ps1`: 2 s idle, walk and turn, jumps, strafe,
flicks; 40 s of settling after launch), `window.present_mode` `immediate` for "vsync off" (the profile tool's
`uncap_frame_rate` does nothing to the game), 165 Hz display, two runs of every build, the GPU lock file held; a change counts only when it beats the spread of two runs of the same build. Two stale processes
(`gpu_assets`, 500000 CPU seconds, and `render_acceptance`, 15000) were running throughout at about 5 cores between them
and the GPU at 1 percent; not started by this work and not killed. The scenario's windows are the recording's, from the
first frame.

Where a frame went before (vsync off; GPU ms are means of the per-pass timestamps, CPU ms per frame on the frame thread):

| | windowed 1280x720 | fullscreen 3440x1440 |
|---|---|---|
| GPU frame mean / p95 | 5.0 to 5.3 / 6.7 to 6.8 | 7.9 to 8.3 / 9.4 to 10.0 |
| Atmosphere | 1.80 to 1.84 | 2.0 |
| Shadow | 0.69 to 0.75 | 0.88 |
| DeferredLight+AtmosphereComposite | 0.37 to 0.45 | 1.96 |
| ObjectBatch, VgBvh | 0.30, 0.27 | 0.34, 0.36 |
| HiZBuild, GBuffer, PostProcess | 0.09, 0.12, 0.10 | 0.44, 0.18, 0.23 |
| CPU: `Game::acquire_surface` (waiting for the GPU) | 4.7 of 8.6 | 7.4 |
| CPU: `encoder.finish` workers (x4 threads) / frame thread's wait | 2.6 / 1.0 | 2.6 / 1.0 |
| CPU: submit, publish host parts, derivations, present | 0.40, 0.38, 0.30, 0.28 | 0.45, 0.48, 0.45, 0.33 |
| editor: first viewport frame (`Helio: rebuild graph`) | 12.3 s on the render thread | |

Experiments, each two runs against the build before it (GPU frame mean ms; windowed, vsync off, unless said):

| change | before | after | kept |
|---|---|---|---|
| GPU scopes in the Atmosphere pass (measure only) | 5.03 / 5.32 | 5.06 (1.87 for Atmosphere, resolve 0.75) | yes (diagnostic, zero cost unprofiled) |
| probe: resolve run twice | | 2.52 for Atmosphere: the second run still 0.56, so the cost is resolve's own | no (probe) |
| the GPU plans the atmosphere (hash, indirect, parallel scan) | 5.0 to 5.3 | 3.35 / 3.29 (Atmosphere 0.23) | yes |
| cascade lattice, coarse but centred on itself | 3.35 / 3.29 | 3.59 / 3.35 (Shadow 0.60 / 0.61) | no |
| open the static pass only with static work | 3.35 | 3.34 / 3.33 | yes, with the next |
| cascade axes from the light, fixed lattice | 3.34 | 2.98 / 3.00 (Shadow 0.17 / 0.28) | yes |
| record before the acquire, copy at the end | period 6.2 to 6.5 | period 3.3 / 3.4 | yes |
| editor viewport builds in the background | first frame 12.3 s | 47 ms | yes |

Spread of two runs of the same build: 0.05 to 0.30 ms of GPU mean (the first baseline pair was 5.32 and 5.03), 2 to 4 fps of
1% low.

Final measurement of round 4, same scenario, GPU / frame figures as the table of the previous section (windowed is the project's
default 1280x720 window, fullscreen is borderless at the display's 3440x1440; "before" is the build of the start of the round
with only the fullscreen setting applied; two runs each):

| configuration | GPU frame mean / p95 ms (runs 1 and 2) |
|---|---|
| windowed, vsync off, before / after | 5.33 / 5.05, p95 6.9 / 6.8  to  2.50 / 2.53, p95 3.6 / 4.1 |
| windowed, vsync on, before / after | 4.57 / 4.92, p95 7.0 / 7.2  to  3.18 / 3.16, p95 4.9 / 4.9 |
| fullscreen, vsync off, before / after | 7.86 / 8.28, p95 9.4 / 10.0  to  6.09 / 6.07, p95 7.7 / 7.7 |
| fullscreen, vsync on, before / after | 7.95 / 7.97, p95 10.1 / 10.0  to  6.19 / 6.08, p95 7.7 / 7.7 |

| configuration | window | frames (run 1 / 2) | period mean ms | p95 | 1% low fps | worst ms |
|---|---|---|---|---|---|---|
| windowed 1280x720, vsync off, before | idle | 379 / 388 | 7.91 / 7.72 | 9.71 / 9.87 | 94 / 92 | 11.31 / 12.56 |
| windowed 1280x720, vsync off, before | walk and turn | 569 / 577 | 9.64 / 9.52 | 11.49 / 11.38 | 80 / 83 | 14.03 / 13.58 |
| windowed 1280x720, vsync off, before | whole 20 s run | 2403 / 2448 | 8.79 / 8.52 | 11.16 / 11.09 | 82 / 84 | 62.92 / 157.58 |
| windowed 1280x720, vsync off, after | idle | 1010 / 1031 | 2.97 / 2.91 | 4.31 / 4.31 | 195 / 207 | 6.49 / 6.64 |
| windowed 1280x720, vsync off, after | walk and turn | 1473 / 1251 | 3.73 / 4.39 | 5.21 / 5.67 | 163 / 156 | 80.67 / 585.38 |
| windowed 1280x720, vsync off, after | whole 20 s run | 3622 / 3583 | 3.36 / 3.62 | 4.88 / 5.22 | 168 / 159 | 80.67 / 585.38 |
| windowed 1280x720, vsync on (165 Hz), before | idle | 493 / 495 | 6.07 / 6.06 | 8.58 / 8.39 | 98 / 104 | 11.71 / 10.94 |
| windowed 1280x720, vsync on (165 Hz), before | walk and turn | 809 / 806 | 6.79 / 6.81 | 10.36 / 10.25 | 91 / 92 | 13.22 / 14.35 |
| windowed 1280x720, vsync on (165 Hz), before | whole 20 s run | 3245 / 3264 | 6.42 / 6.42 | 9.66 / 9.73 | 94 / 92 | 13.22 / 14.35 |
| windowed 1280x720, vsync on (165 Hz), after | idle | 494 / 494 | 6.06 / 6.06 | 6.82 / 6.83 | 125 / 137 | 9.29 / 9.92 |
| windowed 1280x720, vsync on (165 Hz), after | walk and turn | 906 / 872 | 6.06 / 6.06 | 7.11 / 7.15 | 122 / 128 | 9.68 / 8.68 |
| windowed 1280x720, vsync on (165 Hz), after | whole 20 s run | 3091 / 3084 | 6.21 / 6.33 | 7.02 / 7.02 | 123 / 129 | 251.49 / 646.67 |
| fullscreen 3440x1440, vsync off, before | idle | 273 / 257 | 10.97 / 11.64 | 13.53 / 13.66 | 66 / 67 | 15.50 / 15.39 |
| fullscreen 3440x1440, vsync off, before | walk and turn | 463 / 410 | 11.85 / 13.37 | 13.86 / 14.84 | 69 / 63 | 14.92 / 174.07 |
| fullscreen 3440x1440, vsync off, before | whole 20 s run | 1810 / 1695 | 11.57 / 12.31 | 14.06 / 14.28 | 67 / 65 | 16.97 / 344.17 |
| fullscreen 3440x1440, vsync off, after | idle | 493 / 489 | 6.08 / 6.13 | 8.02 / 7.84 | 114 / 115 | 9.35 / 9.55 |
| fullscreen 3440x1440, vsync off, after | walk and turn | 743 / 739 | 7.39 / 7.43 | 9.78 / 9.62 | 94 / 96 | 15.37 / 157.40 |
| fullscreen 3440x1440, vsync off, after | whole 20 s run | 3046 / 3033 | 6.85 / 6.92 | 9.26 / 9.27 | 98 / 98 | 15.37 / 157.40 |
| fullscreen 3440x1440, vsync on (165 Hz), before | idle | 365 / 368 | 8.22 / 8.14 | 13.90 / 12.35 | 67 / 71 | 16.68 / 17.47 |
| fullscreen 3440x1440, vsync on (165 Hz), before | walk and turn | 567 / 557 | 9.67 / 9.87 | 13.11 / 12.86 | 71 / 70 | 15.89 / 15.76 |
| fullscreen 3440x1440, vsync on (165 Hz), before | whole 20 s run | 2351 / 2344 | 8.91 / 8.86 | 13.02 / 12.81 | 69 / 71 | 79.13 / 17.47 |
| fullscreen 3440x1440, vsync on (165 Hz), after | idle | 476 / 478 | 6.30 / 6.27 | 8.20 / 8.42 | 102 / 105 | 11.32 / 11.30 |
| fullscreen 3440x1440, vsync on (165 Hz), after | walk and turn | 750 / 755 | 7.32 / 7.28 | 9.60 / 9.29 | 97 / 98 | 12.02 / 11.91 |
| fullscreen 3440x1440, vsync on (165 Hz), after | whole 20 s run | 2979 / 3002 | 6.97 / 6.93 | 9.18 / 9.23 | 98 / 97 | 12.02 / 11.91 |

Windowed against fullscreen: at the same code the windowed game (0.92 megapixels) is CPU bound at about 3.3 ms and
reaches the 165 Hz refresh with a 1% low of 123 to 129 fps (before: 92 to 94); the fullscreen game (4.95 megapixels) is
GPU bound at 6.1 ms and holds 144 to 146 fps with vsync off or on and a 1% low of 97 to 98 (before: 65 to 71), so it does not
reach the refresh and 165 fps needs the deferred lighting cost at native resolution (issue 17) cut by about a third.
Vsync on and off differ little in fullscreen (the period is GPU bound either way), windowed vsync on is the 6.06 ms refresh
with a p95 of 7.0 ms. The one-off stalls (157 to 646 ms, one in two runs, before and after) are in both modes.

### Final measurement of the performance rounds (round 3, kept for the comparison)

Editor and all 34 plugin libraries rebuilt into their own install (`just plugins`, i.e. `plugin_builder -- --release --out <root>`); Build & Run on a private copy
of the voxel project packed, built and launched the game through the MCP tools (34 libraries found, 34 loaded, none failed);
the game it built then ran the scripted scenario twice (165 Hz display, vsync, the character spawned). Frames are
`Game::Frame` periods; the scenario's windows are approximate (the recording starts about a second before the script).

| window | run | frames | period mean ms | p95 | 1% low fps | worst ms |
|---|---|---|---|---|---|---|
| first 14 s of the process | 1 | 1851 | 7.00 | 12.40 | 44 | 119.5 |
| first 14 s of the process | 2 | 1909 | 6.85 | 12.52 | 49 | 91.1 |
| idle (0 to 3 s) | 1 / 2 | 496 / 496 | 6.06 / 6.06 | 8.05 / 8.16 | 113 / 110 | 9.1 / 10.4 |
| walk and turn (3 to 9 s) | 1 / 2 | 875 / 871 | 6.84 / 6.88 | 10.3 / 10.3 | 93 / 92 | 11.8 / 12.1 |
| jumps while walking (9 to 12 s) | 1 / 2 | 489 / 493 | 6.13 / 6.08 | 8.47 / 8.47 | 104 / 106 | 11.0 / 10.8 |
| strafe and turn (12 to 15 s) | 1 / 2 | 402 / 404 | 7.47 / 7.43 | 9.10 / 9.51 | 99 / 96 | 11.0 / 11.6 |
| fast flicks (15 to 18 s) | 1 / 2 | 494 / 429 | 6.07 / 6.99 | 8.43 / 8.40 | 106 / 112 | 10.6 / 413 (one stall) |
| whole 20 s play run | 1 / 2 | 3228 / 3157 | 6.46 / 6.59 | 9.21 / 9.39 | 96 / 94 | 11.8 / 413 |

GPU frame 4.84 / 4.79 ms mean (p95 7.0): Atmosphere 1.69 / 1.66, Shadow 0.64 / 0.61, DeferredLight with the atmosphere composite
0.43 / 0.34, ObjectBatch 0.24, VgBvh 0.18 / 0.17, GBuffer 0.14 / 0.11. Input to present 10.2 / 10.6 ms mean (p95 15.9 / 16.0;
wait for the tick 3.1, wait for the frame 3.4 to 3.7, render submit 3.2), mouse look to present 5.1 ms mean (p95 7.9, 233 looks). World age
at the frame's read 2.5 / 2.0 ms mean. Frames over the 6.06 ms refresh are the ones whose GPU work does not fit (the walking and
turning windows: Atmosphere to 2.5 ms and the sun's shadow cascades redrawn, a GPU frame p95 of 7.0 ms), not the frame thread (CPU 3.2 ms a
frame: `acquire_surface` waits the rest). Not comparable with the earlier tables of this section: those games had no player character.

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

- **A mesh collider named by path is not simulated** (only an object's own geometry is; see "Collision with
  a level's geometry"). The voxel project's default level still uses one flat box.
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
