# Physics Playground

A small arena that exercises the real physics components of the physics
plugin (`plugins/vendor/physics_plugin`): `RigidBodyComponent`,
`ColliderComponent` (the hitbox), `JointComponent` and `ForceFieldComponent`,
with the level's gravity and `physics_enabled` set in its `world_settings`.
Nothing is mocked; every component property in the level has an explicit value.

## Run it

All commands run from the Pulsar-Native repository root.

### 1. In the editor (then press Play)

PowerShell or bash:

```bash
cargo run --release -p pulsar_engine -- --project-path assets/examples/physics_playground
```

Open the level (`scene/default.level`, it loads on open), then press **Play**.
The editor's Physics section in World Settings shows the level's gravity and
`physics_enabled`.

### 2. As a built game

Package the project (cooks the level and assets into `Content/`, builds the
game executable; the first build compiles the whole engine and is slow).
`--loose` writes `Content/` as plain files instead of `game.pak`.

PowerShell:

```powershell
cargo run -p pulsar_package --bin pulsar -- package --project assets/examples/physics_playground --out build/physics_playground --loose
.\build\physics_playground\physics_playground_game.exe
```

bash:

```bash
cargo run -p pulsar_package --bin pulsar -- package --project assets/examples/physics_playground --out build/physics_playground --loose
./build/physics_playground/physics_playground_game
```

(The executable is named after the project's generated binary, `*_game`; the
packager prints its path. Add `--headless` to the game for a windowless run.)

To cook the content only, without building the executable, add `--skip-build`.
The game opens the cooked startup level (`scene/default.level`) with a
spectator `CameraComponent` placed in front of the arena.

### What the cook produces

`pulsar package ... --loose --skip-build` writes (verified):

```
<out>/Content/
  scene/default.level              the cooked level (editor-only data dropped)
  Pulsar/project.json              cooked project settings, startup level
  Pulsar/asset_registry.json       every shipped asset
  Pulsar/class_index.json          script classes (none in this project)
  assets/meshes/primitives/
    SM_Cube.fbx  SM_Cylinder.fbx  SM_Sphere.fbx    the built-in primitives it draws
```

Without `--loose`, the same files are packed into `Content/game.pak`.

## What is in the level

| Object(s) | Components | Shows |
|---|---|---|
| Ground, Wall North/South/East/West | Box `ColliderComponent` + `RigidBodyComponent` **Static** | static world geometry (layer `World`) |
| Pyramid Crate 1-15 | Box collider (`wood.physmat`) + **Dynamic** body, 5 kg | a stack of 5+4+3+2+1 crates settling |
| Bouncy Ball 1-3 | Sphere collider (`rubber.physmat`) + Dynamic body | dropped from 10-16 m |
| Falling Capsule | Capsule collider (axis Y) + Dynamic body | dropped tilted; drawn as a cylinder and two sphere caps |
| Ramp | Static box rotated 20 degrees (`ice.physmat`) | an incline |
| Rolling Ball | Sphere collider + Dynamic body | starts on the ramp, rolls into the zone |
| Finish Trigger Zone | Box collider with `is_trigger`, Static body (layer `Triggers`) | `trigger_enter` when the ball rolls in (drawn as a translucent green box) |
| Updraft Field + Feather Ball | `ForceFieldComponent` Directional, linear falloff | a light ball is held aloft |
| Gravity Well + Well Ball 1-2 | `ForceFieldComponent` Radial, quadratic falloff | balls pulled to the centre |
| Pendulum Pivot + Pendulum Bob | `JointComponent` **BallSocket** (bob to pivot, 3.96 m) | a pendulum swinging from the side |
| Hinge Post + Swing Door + Door Ball | `JointComponent` **Hinge** about Y, limits -90 to 90 degrees; the ball has a `linear_velocity` | a door a thrown ball knocks open |
| Sun, Sky, Spectator Camera | `LightComponent` (directional), `AtmosphereComponent`, `CameraComponent` | lighting and view |

`world_settings`: `gravity = [0, -9.81, 0]`, `physics_enabled = true`.
Physics assets are in `assets/physics/`: `concrete`, `wood`, `rubber` and `ice`
`.physmat` (different friction and restitution) and `playground.physlayers`
(`Default`, `World`, `Props`, `Triggers`).

Not included, on purpose:

- **Kinematic moving platform:** nothing here drives an object's transform
  (no script or animation in this project).
- **`CharacterControllerComponent` pawn:** it needs input mappings and a
  game-mode pawn role, more setup than an example should carry.
- **Gameplay script reacting to physics events:** a Blueprint class could not
  be authored and verified without the editor. Events can be watched from a
  script in your own class (`physics::collision_enter`, `physics::trigger_enter`,
  ...; see the plugin's README).

Known limit of the engine today (physics plugin README, "Current limits"):
`.physmat` and `.physlayers` are authored and referenced by the colliders but
the simulation does not apply them yet (Rapier defaults for friction and
restitution), and the cook does not ship them into `Content/`.

## How the level is made (and regenerated)

`scene/default.level` and `assets/physics/*` are generated from the real
component types by a test, so every property is written explicitly:

```bash
cargo test -p pulsar_package --test physics_playground generate_physics_playground -- --ignored
```

Change the level in `crates/core/pulsar_package/tests/physics_playground.rs`
and re-run that. The same file's check cooks this folder into loose
`Content/`, runs it headless for 600 ticks and asserts the crates settled above
the ground, a `collision_enter` and a `trigger_enter` arrived, the pendulum
held its length and the session ended with the physics thread joined:

```bash
cargo test -p pulsar_package --test physics_playground the_example_cooks_and_runs_its_physics
```

## Project layout

```
physics_playground/
  Pulsar.toml                 project file (all installed plugins enabled)
  .gitignore                  editor caches and generated build files
  scene/default.level         the level (startup level)
  assets/physics/             .physmat materials and the .physlayers table
```

The editor and `pulsar package` generate `Cargo.toml`, `src/` launcher files,
`Pulsar/level.json` and `target/` beside these; they are git-ignored.
