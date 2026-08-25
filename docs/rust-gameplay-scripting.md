# Rust Gameplay Scripting — End-to-End Tutorial

*Issue [#653](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/653) · scripting epic, Phase E.*

This walks through the full loop: scaffold a gameplay script crate → spawn its
actor → watch it mutate a live component → edit its logic → hot-reload inside
Play-In-Editor without losing any world state.

Everything below uses only what ships today (no editor menus required).

---

## 1. Open a project

Open your game project in the Pulsar editor (`engine_backend::services::
ensure_core_bootstrap` runs automatically on Play; from a fresh checkout you
can also trigger it by pressing **Start Simulation** once).

The project layout this tutorial assumes:

```text
mygame/
├── Cargo.toml            # generated game manifest
├── Pulsar/level.json     # native prefab spawns
├── assets/meshes/primitives/SM_Cube.fbx   # engine-bundled primitive
└── src/
    ├── main.rs / lib.rs  # standalone entry + PIE shim (generated)
    ├── engine_main.rs    # level bootstrap (generated)
    └── classes/          # Blueprint-graph classes (PBGC output)
```

## 2. Your script crate: `scripts/<game>_scripts`

On the first bootstrap the builder scaffolds a starter crate under `scripts/`
named after your project (`scripts/mygame_scripts`). It contains:

- `Cargo.toml` — depends on exactly ONE engine crate, `pulsar_game`, via an
  absolute path into your engine checkout. The generated game manifest lists
  the same path, so both references compile to one shared copy of the runtime.
- `src/lib.rs` — a documented `Spinner` example plus THE registration entry
  point every generated project calls:

```rust
pub fn register_scripts(game: &mut pulsar_game::tick::TickLoop) -> Result<(), String> {
    game.register_actor::<Spinner>(Spinner { degrees_per_second: 45.0 });
    Ok(())
}
```

**Conventions that matter**

| Convention | Why |
|---|---|
| Register every actor type in `register_scripts` | generated `engine_main.rs` calls it on every build/play |
| Always use the turbofish `register_actor::<Type>(...)` | the level editor scans for that pattern to offer your types in the add-object flow |
| Never call bare `game.actors.register` | only `register_actor` stamps the identity that hot reload matches on |

You can add more crates the same way: create `scripts/<name>/` with a
`[package] name = "<name>"` manifest and a `pub fn register_scripts`; the
builder discovers and wires them automatically (path dep + workspace member +
registration call).

## 3. The actor: behavior shells + live components

Actors are deliberately tiny behavior shells (SceneDB design). All state lives
in the shared world as components on the actor's entity. The starter shows the
canonical shape:

```rust
use pulsar_game::{Actor, Entity, World};
use pulsar_game::scene::{MeshAssetPath, StaticMeshComponent, Transform};

pub struct Spinner { degrees_per_second: f32 }

impl Actor for Spinner {
    fn begin_play(&mut self, entity: Entity, world: &mut World) {
        // Absent-only hydration: give the entity something visible unless
        // the scene already provided it.
        if world.get::<StaticMeshComponent>(entity).is_none() {
            world.insert(entity, StaticMeshComponent {
                mesh_asset: MeshAssetPath::new("meshes/primitives/SM_Cube.fbx"),
                ..Default::default()
            });
        }
        if world.get::<Transform>(entity).is_none() {
            world.insert(entity, Transform::default());
        }
    }

    fn tick(&mut self, entity: Entity, world: &mut World) {
        // Mutate the LIVE component — the same row the renderer reads.
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            transform.rotation[1] += self.degrees_per_second * 0.016;
        }
    }
}
```

Key facts:

- `pulsar_game::scene` re-exports the common component vocabulary
  (`Transform`, `Visibility`, `Name`, `StaticMeshComponent`,
  `LightComponent`, `MeshAssetPath`) — script crates need no other engine dep.
- Mutations go through SceneDB's `Mut` guard, so subscriptions fire exactly
  like editor edits; the mesh-frame maintainer rebuilds and the cube moves on
  the next frame in BOTH the editor viewport and PIE.
- `begin_play` fires at registration time; `tick` fires every frame of phase 2
  of the tick loop.

## 4. Spawn it and see it render

Press **Start Simulation** (F5). The editor builds your project as a cdylib,
embeds it (Play-In Editor), and the guest:

1. adopts the editor's authoritative world (ABI v2),
2. runs generated `engine_main::setup`, which registers every `scripts/`
   crate and spawns `Pulsar/level.json` prefabs — all through
   `TickLoop::register_actor`.

Your spinner's begin_play attaches the cube mesh + transform to its own
entity; the offscreen Helio renderer reads the shared world each frame, so the
Game tab shows a rotating cube.

Prefer spawning per level instead of unconditionally? Add an entry to
`Pulsar/level.json` prefabs (that flow covers `src/classes/` blueprint actors)
or gate `register_scripts` yourself — registration is ordinary Rust code.

## 5. Edit logic → hot reload without losing state

While the game is RUNNING:

1. Edit `scripts/mygame_scripts/src/lib.rs` — e.g. raise
   `degrees_per_second` to `180.0`.
2. Press **Start Simulation** again. The toolbar button becomes the reload
   trigger while a session is active ("Reload Simulation").
3. The editor rebuilds, stops the old game host, and loads the new dylib
   against the SAME shared world.

What survives and why:

| | Survives? | Mechanism |
|---|---|---|
| Entities + all components | yes | the world lives host-side (editor), never in the dylib |
| Actor behavior | updated | fresh code registers through `register_actor` |
| Entity ↔ actor association | preserved | entities carry a `ScriptTag` naming their actor type; the new session re-binds matching registrations to those entities instead of spawning |
| `begin_play` | refires on the SAME entity | hydration is absent-only, so nothing duplicates |

This is the native equivalent of the VM path's `reload_blueprint` (#648).
Details + invariants: `pulsar_game::scripts` module docs.

Notes:

- If the rebuild FAILS, the error notification shows cargo/rustc diagnostics
  with file/line (the old game keeps running until a build succeeds; a failed
  load stops the session and reports why).
- A class you DELETE leaves its entity orphaned-but-intact (data outlives
  behavior); a NEW class spawns fresh. Cleanup policy is future work.

## 6. Compile errors surface where you're looking

Build failures during Play/reload land in the editor notification with the
tail of cargo's stderr (which carries rustc's `file:line:` spans), and the
full log goes through tracing. No separate tooling needed.

---

## Where things live (code map)

| Piece | Location |
|---|---|
| Registration + reload rebinding | `crates/core/pulsar_game/src/scripts.rs` |
| Tick integration (rebound shells) | `crates/core/pulsar_game/src/tick.rs` (phase 2) |
| PIE reload flag (ABI v3) | `crates/core/pulsar_pie_abi/src/lib.rs` (`session_flags::RELOAD`) |
| Host-side swap-on-reload | `game_viewport.rs` `drive()` / `pie_host.rs` |
| Scaffolding + manifest wiring | `engine_backend/src/services/core_project_builder.rs` (`ensure_scripts_crate`, splice/workspace helpers) |
| Crate/type discovery | `engine_backend/src/services/native_scripts.rs` |
| Editor data model (rust-mode binding record) | `ui_level_editor/src/level_editor/state/native_scripts.rs` |

## Known limits / follow-ups (Phase F+)

- Add-object menu UI for discovered Rust actor types (data model +
  discovery shipped; no menu yet).
- Per-object actor bindings do not drive play-time spawning yet — the
  rust-mode `ScriptComponent` record is written/preserved but needs a
  level-format section (mirror #650's `blueprint_bindings`) plus F's inspector.
- VM-blueprint instances are not re-bound across a native reload session
  (they have their own `reload_blueprint` path; wiring it into PIE remains
  editor work from D5/#650).
- Time is currently a fixed-step approximation in actors (`Actor::tick` is
  time-free by contract); convert via `pulsar_game::time::to_scenedb_time` if
  you drive timing from systems/events.
