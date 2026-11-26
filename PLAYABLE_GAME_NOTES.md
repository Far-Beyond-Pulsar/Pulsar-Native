# Can we make a game with Blueprints? Coin Town field notes (2026-11-26)

We built a coin-collection game on the Bistro street (`voxel-reference-project`) with `just run-linked`, driving the
editor over its MCP HTTP server and looking at the UI only now and then.

**Verdict:** a very small game loop is possible today. It is not yet practical for a game developer: the core nodes
for player feedback are missing, several failures are silent, and the MCP tools cannot author a Blueprint from start
to finish.

Each item is marked **seen** (reproduced in this session) or **code** (found by reading source, not run).

## 1. What was built

| Piece | Where (in the project) |
|---|---|
| Level: copy of the Bistro with 8 coins | `scene/coin_town.level` (the original is untouched) |
| `Coin.class`: spins; a sphere trigger reports who walks in; scores and removes itself | `classes/Coin.class` |
| `CoinGameMode.class`: keeps the score, announces the win at 8 | `classes/CoinGameMode.class` |
| `CoinPlayer.class`: the default character with faster walk and sprint | `classes/CoinPlayer.class` |
| Startup level | `Pulsar/project.json` (`startup_level`) |

Also left in the project: `.pulsar/project/project.toml` (the ignored `default_map` attempt, safe to delete), and
`.pulsar/layout.json` was deleted between restarts.

**Verified in the packaged game (from its log):**

- Spawned on a coin, the "On Component" trigger collects it and the game mode logs `score 1`, with no script errors.
- Walking into coins collects them. This was seen in the polling version and a trigger stress run; we sent no input,
  so the user was presumably playing.
- The win message appears at 8 coins (seen in the 508-coin trigger run; it repeats on later coins, which a level
  with 8 coins never reaches).

**Not verified:** look, jump and sprint as such, and a full clear of the 8-coin level in the final build. MCP has no
input injection and desktop control of the game window was declined. The first version polled the player's distance
from every coin; it was replaced by physics triggers after the fixes in section 3.

## 2. MCP blockers fixed (code changes in two plugin submodules)

1. **Every `blueprint_*` tool was dead** (seen). The material graph editor registers a copy of the Blueprint tools under
   the same names, and the tool bridge keys tools by name (`plugin_manager/src/tool_bridge.rs`), so the copy silently
   won. Renamed to `material_graph_*` (`plugins/vendor/materials`).
   *Still open:* the bridge should reject or warn on duplicate names. Check that nothing in the chat panel relied on the
   old material names.
2. **Calls without `file_path` could never work** (seen): the bridge passes a placeholder path that the Blueprint
   tools looked up literally. They now fall back to the last used Blueprint.
3. **Reads returned an empty graph** (seen): the AI session copied the graph once, before the canvas existed. Each call
   now reads the live canvas, and reports an error if it is not built yet.
4. **Added `blueprint_search_node_definitions`** (there was no way to discover node ids) **and `blueprint_save`**
   (edits could not be persisted). `blueprint_save` only reports that the save started.

Tests were not run for these changes; they were checked live over MCP.

## 3. Physics triggers: three engine bugs, fixed

The collider already had `is_trigger` and `trigger_enter` / `trigger_exit` events, but a pickup could not work
(all in `plugins/vendor/physics_plugin`):

1. **No events at all** for a character entering a pickup. The capsule is kinematic and a body-less sensor is fixed,
   a pair Rapier skips by default. Triggers now ask for kinematic-fixed and kinematic-kinematic pairs.
2. **A trigger was an invisible wall** (reproduced: the character stopped at the sensor's surface). The character's
   movement query now excludes sensors.
3. **The UI's "On Component" node could never fire.** It listens on the component instance's channel, and physics
   published only on the owning object's. Physics events now go to both.

A new test, `a_character_walks_through_a_pickup_trigger_and_is_reported`, failed before the fixes; all 70 physics
tests pass after. Fix 3 was also verified in the packaged game. The running editor predates these fixes, so
Play-in-Editor needs a rebuild (`just run-linked`); packaged games pick them up on their next build.

Still open: the shipped `AdvPickup` example relies on the generic `On BeginOverlap`, which nothing publishes (code).
The "On Component" node cannot be added through `blueprint_add_node`.

## 4. Frame rate: polling against triggers

Packaged release game, 508 spinning coins, player idle at the same spawn, GPU frame 5.19 ms in both runs (the same
view). One run each; the profiler keeps 1,000,000 events, so each recording covers only about 3.3 s.

| | Polling | Physics triggers |
|---|---|---|
| Mean fps / 1% low | 145.5 / 97 | 145.5 / 95 |
| Mean frame | 6.88 ms | 6.87 ms |
| Simulation tick, mean | 3.37 ms | 3.04 ms |
| Scripts per tick | 2.67 ms | 1.97 ms |
| Physics step on the tick thread | 0.19 ms | 0.32 ms |
| Physics thread per frame | 0.04 ms | 0.13 ms |

- **The polling design did not change the frame rate** at this size: rendering sets the frame time and the simulation
  runs on its own thread. It cost simulation time: the distance handler is about 1.1 us per coin (0.56 ms a tick),
  and grows with coins times players.
- Triggers made the tick **0.33 ms cheaper**, net of extra physics work.
- **The biggest cost in both is the spin**: about 1.4 ms of `Coin::tick` plus 1.5 to 1.7 ms of script write-back. A
  built-in rotate component, or animating in the material, would matter more than the trigger.
- Part of the extra physics cost is probably the spin too: rotating a coin moves its sphere collider every tick,
  though a sphere does not change.
- With 8 coins neither design is measurable. An early polling run (148.6 fps, 114 1% low) and a trigger run at
  vsync (165 fps) had different views, so do not compare them. A first 508-coin trigger run read 92 fps because the
  player was walking and the GPU frame was 9.1 ms; it was rerun.

## 5. Missing MCP / AI tools

These are the calls we had to replace with hand-written files, UI clicks or guesswork. In priority order:

| Need | What we did instead |
|---|---|
| Create a class or other asset (class, level copy, input action, material) | wrote folders and JSON by script |
| Add **dynamic nodes**: variable get/set, custom events, On/Send/Broadcast/Send-to-Class, component and input events. `blueprint_add_node` returns "Unknown node definition_id" (seen) | wrote `graph_save.json` by script |
| Variables, custom events and traits: create, edit, delete | same |
| Class components (prefab): add, set properties | same |
| Compile **one** class and get node-level diagnostics | ran a full game build (about 3 minutes to learn of 2 errors) |
| Blueprint undo/redo | none |
| World settings: roles, `player_start`, input mapping | edited the level JSON |
| Project settings (startup level) | wrote `Pulsar/project.json` |
| List open tabs, focus a tab, close a tab; level "loading" status | clicked in the UI; polled until the scene answered |
| Viewport camera get/set and focus object | moved the coins to where the saved camera already looked |
| Object and mesh bounds, raycast / drop to ground | guessed positions and scale from screenshots |
| Input injection into PIE or the game, and a game screenshot | could not play-test |
| Game state inspection (actors, script variables) | log lines only |
| A script-log filter that survives the flood (see 7) | paged `build_output` with a throwaway script |
| Create or assign a material | tried raw component JSON; no visible change |

Also: `blueprint_search_node_definitions` does not list the dynamic event nodes either.

## 6. Missing engine and Blueprint features

| Feature | Evidence |
|---|---|
| **HUD / on-screen text / widgets at runtime** | seen: no nodes for "hud" or "widget". The score exists only in the log |
| ~~Audio~~ | added: the audio plugin (section 11) |
| **Load or restart a level, quit** | seen: no nodes |
| **Get Player Character / find by class or tag** | seen: none. The player has to broadcast itself every tick |
| **Float comparison** | seen: `less_than` is integer-only; "distance < 2 m" needs a float-to-int conversion |
| **`Vec3` as an event field** | seen compile error: only bool, integers, floats, String, Entity |
| Hide or show a mesh | code: no script call for visibility |
| Generic `On BeginOverlap` / `On Hit` | code: declared, never published (component triggers work after section 3) |
| Mesh collision | code and docs: triangle-mesh colliders are not simulated; the street is a flat box |
| Spawn with rotation and scale | code: spawn takes class and position |

## 7. Bugs and silent failures

1. **The `Log Info` node prints nothing** (seen). `pulsar_std`'s `log_info` uses `tracing::debug!`, which the game's
   default filter hides. `native::log::info` works. The shipped examples use `log_info`.
2. **`Send <Event> to Class` can silently go nowhere** (seen): a custom event listens on the object's own channel, so
   it only arrives if a handler is set to `class` scope. No compile or runtime warning.
3. **`entity::despawn` leaves the script running** (seen): the removed coin's script failed every tick, thousands of log
   lines. `world::destroy` is correct. The shipped `AdvPickup` uses `entity::despawn`, and both nodes appear in search.
4. **`project/default_map` is ignored** (seen). The packer and the game read it, but no registration of the setting
   was found in the source, so the old level shipped without any warning.
5. **The packaged game crashed once with out of memory** (seen, once in four launches): `memory allocation of 16777216
   bytes failed` in `texture_mips::mip_chain` while cooking textures on the first launch. A later launch peaked at
   16 GB private memory (7 GB working set). The editor held about 5 GB at the same time. Commit exhaustion is suspected,
   not proven.
6. **`SM_Torus` primitive is about 100 times too big** (seen): scale 0.4 covered the street; about 0.006 makes a coin.
   Probably a centimetre import; the other primitives were not checked.
7. **Material change reports success with no visible effect** (seen once, not investigated):
   `level_editor_set_component_properties` on `material_slots.slots[].imported_surface`.
8. **Log floods hide script output** (seen): mesh load, file watcher and render graph lines push tens of thousands of
   lines out of the 20,000-line live buffer within a minute.
9. **Fixed:** physics triggers did not work for characters (section 3).
10. A profile recording keeps 1,000,000 events, so a 20 s request returned about 3 s with 500 scripted actors (seen).
11. The Problems panel reports idle threads (MCP workers, encoder threads, an idle viewport) as "stalled", and many
   "Blocking call on the UI thread" errors (seen).

## 8. First-frame bugs (editors that need a nudge)

Reported by the user, and seen here:

- **Blueprint graph stays blank until the mouse moves over it** (seen, reproduced: blank in a screenshot, drawn after a
  mouse move). Before fix 3 in section 2 this also made MCP report 0 nodes. After the fix the graph read correctly
  without mouse input in the next run; the blank canvas itself was **not** fixed.
- Related, **cause not confirmed**:
  - **Every Bistro level open takes about 120 s** (seen: 119, 120, 120, 128 s). The load waits for pending asset work
    with a hard cap, `LEVEL_ASSET_WAIT = 120 s`, and logs "Scene assets are still loading" when it gives up. Something
    never finishes. A render loop that is not ticking is one candidate. Until then the scene tools cannot see the level.
  - `viewport_screenshot` times out when the level tab is hidden, and nothing over MCP can bring it forward.

Suggested check: open each editor type with the mouse outside the window, screenshot, and require content.

## 9. Barriers in the UI and workflow

- **Every build opens another Build Dashboard window** (seen; reported by the user). They stack exactly on top of each
  other, so closing one looks like nothing happened, and they cover the viewport. One dashboard should be reused.
- **Startup can show the level twice:** an `<unsaved new level>` tab already holds the Bistro content, and opening
  `default.level` adds another (seen). Every scene tool then demands a `file_path`.
- **The saved layout stores absolute paths** (seen): this project restored a level from a sibling folder,
  `voxel-reference-project shot`, so edits could land in another project. The same level also opened twice because the
  two paths used different separators.
- **A closed level tab stayed registered** with the MCP tools (seen once).
- **Loading looks finished or absent:** while loading, tools say the level is `<unsaved new level>` or not open, with no
  progress state.
- **Iteration is slow** (seen): about 12 s per Blueprint class at pack time, including unrelated demo classes, and about
  2.5 to 3 minutes from a graph change to a running game. Play-in-Editor was not tried.
- **No editor field for `player_start`** (docs). With no key, the player starts at the origin, far from the visible
  plaza.
- Placing something where you are looking needs the camera pose, which tools cannot read (seen).
- The window opens small on an ultrawide display once the saved layout is removed.

## 10. Suggested order of work

1. Add tests for the section 2 fixes. Then silent failures: `Log Info` level, send-to-class warning, despawn with a live script, `default_map`.
2. First-frame redraw and the 120 s load.
3. MCP authoring: create class, dynamic nodes, variables and events, compile one class with diagnostics, world
   settings, camera, tab control.
4. Gameplay nodes: HUD text, player lookup, a tested trigger event, float math, audio, restart and quit.
5. MCP play-testing: input injection, game screenshot, state query.

## 11. Added after the first round: audio, particles, viewport icons

`plugins/vendor/audio` and `plugins/vendor/view_icons` are submodules (Plugin_Audio, Plugin_ViewIcons).

### Audio (`pulsar.audio`, new)

- `AudioSourceComponent` (positional, distance falloff and stereo pan), `AmbientSoundComponent` (everywhere or a zone)
  and `SoundEffectComponent` (no position), each with `play` / `stop` Blueprint methods. WAV, Ogg, MP3, FLAC.
- A registered service thread owns the device, decodes and mixes (64 voices). Voices hold while the game stops ticking.
  A sound asked for by an object destroyed in the same script still plays.
- Engine seam: the tick publishes the active view as a session resource (`ViewPose`), which is the listener.
- Seen in the packaged game's log: device open, ambience and birds looping, coin chime on pickup.
- **Not verified: how it sounds** (no one has listened). Limits: whole files are decoded into memory (no streaming);
  no buses or volume settings; no editor preview; no free `play sound` node, so a sound needs a component.

### Particles (existing plugin, fixed)

- Slow emit rates now work (the fraction carries over). Emitters keep stable pool slots, so changing one no longer
  restarts others. New `burst(count)` and `set_spawning(bool)` methods and an `emitting` property. Sphere spawning is
  uniform. Disabling or removing an emitter still clears it at once.
- The GPU acceptance test `particle_emitters_reach_the_frame_from_one_pool` passes. Fireflies and leaves are seen in
  the editor. **Not seen: the pickup burst** (it only happens in the game window).
- Still missing: box and cone shapes, drag, wind or noise, spin, additive blend, soft particles, user textures,
  sorting (the sort code is dead), fog and anti-aliasing correctness. 64 emitters at most, and each coin uses one.

### Viewport icons (`pulsar.view_icons`, new)

The billboard pass moved out of the lights plugin. A plugin registers an image and publishes rows under its own key
(`ViewIconImageRegistration`); the pass draws each kind with its image. Audio has three speaker images; lights keep
the bulb. Seen in one editor screenshot with all four. Clicking picks a pure sound object by its icon (unit tested,
not clicked). Icons are placed only by the editor's renderer. The portal billboard test passes.

### Pickup feedback in Coin Town

On the trigger a coin scores, plays its chime, bursts 60 particles, shrinks away and destroys itself 1.2 s later. The
level also has a town ambience, birdsong at the tree, fireflies and falling leaves. Sounds were synthesized by script
(`assets/audio/` in the project).

### Test state

Passing: audio (component and plugin), viewport icons, particle GPU test, billboard portal test, preset pass lists,
installed set and contracts.

Failing, in files this work does not touch, **not compared with a clean checkout** unless noted:

- `editor_task_queue` raw-thread ratchet (`thread_stack.rs`).
- `one_owner` (a lens component string).
- `scene_inventory` (world registry state count; missing ledger rows for cables and several passes).
- `hack_gen --check` (drift in SceneDB's `telemetry` feature).
- particles `graph_node` (confirmed failing without these changes).
- lights `graph_node` (the HLFS preset includes light culling).
- `installed_plugins` `a_disabled_plugin_is_absent` (black cube; still black with viewport icons disabled).

## 12. More tool and UI gaps found in this round

- **Asset properties added by new components are plain text boxes** (reported by the user): the audio components'
  `sound_asset` has no asset picker. All components' asset references need an audit.

- `viewport_screenshot` can capture the "Building the render pipeline" screen instead of waiting for the scene.
- Particles appear in `viewport_screenshot` only once they have built up; right after placing an emitter there are none.
- The level load took about 22 s on the last two launches against about 120 s before. Cause unknown.

