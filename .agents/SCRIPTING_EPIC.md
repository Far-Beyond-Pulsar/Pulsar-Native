# Scripting Epic — Agent Pipeline Context

Master context for the six sequential implementation agents (A→F) building
Pulsar-Native issue #516 to the finished product. Epic: #633. Every agent
MUST read this file first, then its own letter's section, then the GitHub
issues for its letter.

## Architecture target (invariant across all agents)

Gameplay logic — Rust crates or Blueprint graphs, compiled to VM bytecode or
native Rust via PBGC — manipulates the world exclusively through lightweight
references (`ActorRef` / `ComponentRef`) that resolve into **SceneDB**
(`pulsar_scenedb::World`). SceneDB is the only runtime world state; editor,
renderer, PIE, standalone games, and scripts share it. All property/method
dispatch funnels through Pulsar-Reflection metadata + caller closures.

Key upstream facts (verified Aug 2026):

- SceneDB pin `c761890`: Subsystems API merged (`Subsystem::simulate_a/b`,
  `SubsystemRegistry::dispatch`, `SceneDb::step()` — already called every
  frame by `HelioRenderer` at `renderer.rs:1562`).
- Subscriptions shipped (#47): `subscribe_id(entity, ComponentId)` → batched
  `take_component_change_events`; `Mut` guard fires events only on real
  writes.
- Reflection has method metadata + caller closures
  (`MethodMetadata.caller`, `REGISTRY.get_method`) but zero consumers yet.
- `Entity` = packed slot|generation u64, slots recycled on despawn;
  `StableId(String)` is save/load identity (`world_store.rs:14-22`).
- The properties panel (#519/#575) established conventions this epic adopts:
  component identity `(class_name, component_index)`;
  live-typed-vs-metadata routing; subscribe-once/cache-until-signaled UI.

## Execution order and inter-agent contracts

Agents run strictly in sequence. Each appends a handoff report to the end of
this file under `## Handoff: <letter>` describing exactly what landed, what
was stubbed, and what downstream letters must know. Each agent must ALSO read
the previous handoffs before starting.

- **A (one world)** delivers: play mode reading/hydrating SceneDB; TickLoop on
  the shared store; lights/mesh residency improvements; PIE ABI v2 direction.
  Downstream promise: there exists ONE world handle pattern
  (`Arc<RwLock<WorldSceneStore>>` / `SceneDb`) that B–F code against.
- **B (object model)** delivers: `ActorRef`/`ComponentRef` value types,
  StableId↔Entity resolution service, never-panic error taxonomy,
  Reflectable identities. Downstream promise: one crate/path C, D, E, F use
  for all identity handling.
- **C (reflection core)** delivers: `invoke_component_method(...)` dispatcher
  in `pulsar_world_registry` + unified JSON⇄Any⇄bytes marshalling + metadata
  audit. Downstream promise: D/E call THIS api, never bespoke dispatch.
- **D (VM path)** delivers: comp_* opcodes, executor context reaching the
  world, per-entity dispatcher instances, level.json bindings format.
  Downstream promise: F can assume graphs compile+run against live state in
  the VM target.
- **E (Rust path)** delivers: sourcegen actors using live World via C's API,
  signature-drift fix + CI compile check, script-crate authoring UX.
  Downstream promise: generated code style is stable for F's node UX.
- **F (editor UX)** delivers: cross-object reference pins/nodes surviving
  codegen, bidirectional palette filtering, validation stage + Problems
  panel. Consumes everything above.

## Quality bar (NON-NEGOTIABLE, every agent)

1. **File organization**: one concern per module. Target < ~300 lines/file;
   split earlier when a file accumulates distinct responsibilities. New
   crates follow the repo flat-layout convention (see `.agents/UI_CRATES.md`,
   `.agents/FILE_MANAGER.md`). Name modules by domain nouns, not "utils"
   dumping grounds.
2. **Structure**: public API surfaces get `///` doc comments stating purpose
   and invariants (repo style); internals stay `pub(crate)` unless needed.
   Errors are typed enums with `thiserror`-style Display, never `String`.
   No god-objects: pass narrow contexts, not whole app state.
3. **Consistency**: mimic surrounding code (naming, GPUI patterns, existing
   registries). Reuse before inventing: subscriptions, `Mut` guard hooks,
   `pulsar_world_registry` bridges, #519/#575 panel conventions.
4. **Tests**: every new module gets unit tests next to it; integration tests
   for cross-crate contracts listed in each issue's acceptance criteria.
   Regression tests reference their issue number in the test name/doc.
5. **Verification before handoff**: `just check` clean; `just test` no NEW
   failures (pre-existing: two `toggle_button.rs` doctests fail on main;
   workspace clippy has pre-existing warnings — do not fix unrelated ones,
   but touched files must be warning-clean).
6. **Commits**: one commit per issue (`#NNN: imperative-mood summary`),
   on branch `scripting-epic`. Never push. Leave the tree green.
7. **Honesty**: if an issue cannot be completed, stop cleanly, mark the
   remaining checklist items clearly, and document why in the handoff —
   a smaller finished slice beats a sprawling half-broken one.

## Repository folder inventory (agents have NO prior knowledge of these)

All work happens in **`D:\GitHub\Pulsar-Native`** (canonical checkout,
branch `scripting-epic`). Reference repos are **READ-ONLY** — never push,
never edit their files. If an upstream change seems required, build a thin
local shim inside Pulsar-Native instead and record the needed upstream
change in your handoff.

| Path | Role | Relevance |
|---|---|---|
| `D:\GitHub\Pulsar-Native` | The engine. ALL code changes here. | Primary |
| `D:\GitHub\Pulsar-Native\crates\renderer\helio` | Helio renderer **submodule** (at `2d7960e9`, has uncommitted user change — do NOT touch submodule pointers or commit inside it) | Read-only reference |
| `D:\GitHub\Pulsar-Native\plugins\vendor\*` | Vendored plugin checkouts (blueprint_editor, shader_editor, …). Separate git repos compiled into the build. Commit inside them only when a signature change forces it; parent pointers stay untouched. | As needed (D/F) |
| `D:\GitHub\SceneDB` | `pulsar_scenedb` source. Local main `2ceb8a9` is AHEAD of the workspace pin `c761890` (Cargo.toml:94) — read newer code for context but write against the pinned API. | Read-only reference |
| `D:\GitHub\pulsar_reflection` | Reflection registry (`EngineClass`, `MethodMetadata.caller`, `DYN_METHOD_REGISTRY`). Local `745ee78` == the pinned rev. | Read-only reference |
| `D:\GitHub\Template-Blank` | Sample game-project template ("mygame") consumed by core_project_builder scaffolding. | E3 |

Explicitly NOT relevant (do not explore): `Nightly`, `WGPUI*`/`wgpui-p*`,
`gpui-component`, `blog*`, `Pulsar-Installer`, `Helio-cpp`, `helio-ffi`,
`Helio_Standalone_Demo`, `helio-imgui-test`, `Quasar`, `Studio`, `Nebula`,
`Research`, `db`, `mc`, `test`, `wiremann`, `crab_tunnel`, `Solid3D`,
`PiFrost-Infra`, `Documents-Migrator`, `profiling`, `Plugin_*`,
`ue4-sample-project`, `UI`, `far-beyond-pulsar.github.io`,
`tristanpoland.github.io`.

Environment notes for agents:

- Windows + PowerShell. No `head`, no ripgrep CLI — use the Grep/Glob/Read
  tools. Quote paths with spaces. Long cargo builds: pass generous timeouts.
- `gh` CLI is authenticated; fetch your letter's issues with
  `gh issue view <n> --repo Far-Beyond-Pulsar/Pulsar-Native --comments`.
- Pre-existing failures you must NOT chase: two `toggle_button.rs` doctests
  fail on clean main; workspace clippy warns on many untouched files.
- The helio submodule shows as modified (`2d7960e9`) — that is the USER's
  intentional change. Leave it out of every commit.

## Handoff: A

Branch `scripting-epic`, commits in order (one per issue, helio submodule
pointer excluded from all of them):

| Commit | Issue | Summary |
|---|---|---|
| `cba933d8` | #636 | Light world-transform folded into SceneDB-resolved frames |
| `06e1ccb4` | #637 | Play-mode scene bootstrap hydrates into WorldSceneStore/SceneDb |
| `8f9d9ceb` | #634 | TickLoop mutates the shared store; parallel World deleted |
| `25e17c8d` | #635 | PIE ABI v2: shared-world token + lock-witness protocol |
| `a0da33d8` | #638 | Mesh instance frames subscription-maintained; material protocol documented |

### What landed per issue

**A5 / #636 — light residency.** New module
`crates/core/engine_backend/src/scene/light_frame.rs`:
`ResolvedLightFrame { gpu_light: GpuLight }` is a World component carrying the
fully-combined per-light GPU state (mirror translated via
`to_helio_gpu_light()` + live `Transform` position folded into
`position_range[0..3]`). `LightFrameMaintainer` arms SceneDB#47 subscriptions
on `(entity, LightComponentGpuMirror)` and `(entity, Transform)` for every
mirrored light, prunes on mirror-row disappearance, seeds new lights, and
re-resolves only changed entities. `HelioRenderer::rebuild_light_frame`
(`engine_backend/src/subsystems/render/helio_renderer/renderer.rs`) now reads
those rows only; the TRANSITIONAL per-frame CPU combine is deleted.
Absence semantics preserved: light renders iff it has BOTH a mirror row AND a
Transform.

**A1 / #637 — play-mode bootstrap.** New modules in
`engine_backend/src/scene/`: `runtime_level.rs`
(`RuntimeLevel::load`/`load_into`/`from_scene_file`,
`RuntimeLevelError`, `EditorCamera`; parses the canonical
`pulsar_scene::format::SceneFile`, maps objects to `ObjectSnapshot`s, inserts
via `WorldSceneStore::insert_snapshots`, hydrates registered classes via
`pulsar_world_registry::hydrate_world_component_for_class` — persisted
top-level `components` map authoritative over inline `component_instances`,
unregistered classes stay JSON in `RenderProps`) and `helio_bridge.rs`
(`attach_gpu_render_seam`, `rebuild_static_mesh_frame`, `rebuild_light_frame`,
`step_scene_for_render` — the World↔renderer operations extracted OUT of
`HelioRenderer` so editor and play modes run identical code).
`pulsar_scene::SceneLoader` is `#[deprecated]` (import/legacy conversion
only); both former runtime callers converted. Camera selection:
`pulsar_game::camera_selection::select_world_camera` renders from
`ObjectType::Camera` entities' Transforms in the shared world (precedence:
WindowBridge camera > world Camera entity > freecam). Callers must set
`engine_state::set_project_path` BEFORE hydration so mesh assets resolve.

**A2 / #634 — one tick loop.** `TickLoop.world` (owned empty
`pulsar_scenedb::World`) is DELETED; replaced by
`TickLoop.scene_store: Arc<RwLock<WorldSceneStore>>` (`pulsar_game/src/tick.rs`).
`tick_once` takes the write lock once per phase (schedule → actors →
blueprint events), dropping it between phases; blueprint dispatch touches no
World. The GameTime dual-type conversion is now a named, documented seam:
`to_scenedb_time()` (orphan rule forbids `From`; pulsar_core deliberately has
no scenedb dep). `WorldSceneStore::insert_snapshots` = additive snapshot
insertion into a LIVE store (extracted from `load_from_snapshots`);
`RuntimeLevel::load_into` uses it so setup-time actors survive level load.
`core_project_builder.rs`'s generated `engine_main.rs` now emits
`game.actors.register(actor, &mut game.scene_store.write().world());`.
Gameplay path grep-clean of `World::new()` (unit tests excepted).

**A3 / #635 — PIE ABI v2.** `PIE_ABI_VERSION` bumped to **2**.
`EngineContext` gained (appended): `shared_world: *const c_void` (the host's
Arc-backed `RwLock<WorldSceneStore>`), `lock_shared_world`/`unlock_shared_world`
callbacks taking the context's userdata. Locking protocol + FFI safety
DECISION are stated in `pulsar_pie_abi`'s module doc: direct exclusive
reference under a non-reentrancy witness (null on double-lock), command queue
documented as escape hatch if a future guest breaks single-threaded slices.
Host: `PieWorldBridge` (`engine_backend/src/services/pie_host.rs`) implements
the callbacks via a slot-held `'static` guard backed by an owned Arc clone;
`PieHost::load` now takes `shared_world: Arc<RwLock<WorldSceneStore>>`
(`game_viewport.rs` passes `SceneDatabase::shared_store()` — NEW accessor),
transfers ONE count to the guest via `into_raw`, reclaims it if init fails,
drops the bridge after shutdown. Guest (`pulsar_game/src/embed.rs`): reclaims
with exactly one `Arc::from_raw`, builds `TickLoop::with_scene_store(...)`
(NEW constructor), SKIPS level-file hydration (world comes pre-hydrated) and
skips freecam seeding (world-camera preferred). Two host-side tests cover the
witness + aliasing.

**A4 / #638 — mesh instance frames.** New module
`engine_backend/src/scene/mesh_frame.rs`: `ResolvedMeshFrame { model,
normal_mat, position, bound_radius, visible }` + `MeshFrameMaintainer` —
identical pattern to lights, subscriptions on
`(StaticMeshComponent, Transform, Visibility)`; missing row == not rendered
(preserves the old join). `helio_bridge::rebuild_static_mesh_frame` reads
resolved rows and combines them with FRESH pool handles (mesh_key/draw params
are never cached — pool regrow shifts offsets). `step_scene_for_render` now
maintains both maintainers; `HelioInner` carries both.
The #638/Helio#231 ownership+invalidation protocol is written into
`helio_bridge.rs`'s module doc: geometry = SceneDB pools (Helio borrows);
per-instance state = World resolved rows; materials = renderer-owned table
(Helio#231), World binding component deferred (below).

### Downstream contracts (B–F code against these)

1. **ONE world handle**: `Arc<RwLock<WorldSceneStore>>`
   (`engine_backend::scene::WorldSceneStore`, wrapping `pulsar_scenedb::
   SceneDb`). Clone it to share; readers `.read().world()`, writers
   `.write().world_mut()`. Keep write scopes SHORT (render thread contends);
   never hold across frames or phases. The tick loop owns the canonical
   handle in play mode; the editor's `SceneDatabase::shared_store()` returns
   the same instance in edit mode.
2. **Derived render state lives in the World** as components maintained by
   subscription-driven maintainers: `ResolvedLightFrame` (#636),
   `ResolvedMeshFrame` (#638). If B–F add components that feed rendering,
   extend those maintainers rather than adding per-frame queries.
3. **Component identity/hydration**: class-name keyed via
   `pulsar_world_registry::{hydrate_world_component_for_class,
   remove_world_component_for_class, registered_world_component_classes,
   component_id_for_class}`. Level files carry FULL serialized component
   JSON (hydrates deserialize real structs — sparse JSON fails).
4. **Runtime scene loading**: `RuntimeLevel::load_into(path, &mut store)` —
   additive, actor-preserving. Never construct a second world for a level.
5. **Time**: convert at `pulsar_game::tick::to_scenedb_time` only.
6. **PIE**: guests adopt the host world via ABI v2 token +
   `TickLoop::with_scene_store`; do not reintroduce guest-owned worlds.

### Stubbed / left open

- **#637/#635 acceptance "runtime-spawned entity renders same frame"**:
  architecturally complete (shared store + rebuild path), but END-TO-END
  manual verification inside a running editor/game was NOT possible from this
  agent (no display/GPU session); unit + compile gates are green.
- **#635 checklist**: editor Play flow still writes the temp `.level` and the
  host still passes its path (advisory under v2). Once v1 guests are
  irrelevant, drop the export entirely. Witness-mediated locking for guests
  OUTSIDE the single-workspace Rust universe would need a lock abstraction in
  `TickLoop` — deferred to whoever finalizes B's object-model handles.
- **#638 scope item 2**: per-instance materials by stable id REQUIRE a
  `MaterialComponent` in the World and Helio#231's material table stages;
  `StaticMeshComponent` (read-only submodule) currently has no material field
  at all. Pulsar-Native-side contract + shared-default transitional binding
  documented in `helio_bridge.rs`. When upstream lands, bind via the existing
  subscription mechanism.
- **Template-Blank** (`src/engine_main.rs`) still shows the pre-A2 generated
  shape; it regenerates automatically on next editor blueprint compile
  (generator fixed here). E may want to refresh the template proactively.

### Upstream changes needed (recorded, not made)

- None blocking. (Helio#231 owns the renderer-side material table + instance-
  residency stages; the World-side contract it can code against is
  `ResolvedMeshFrame` + `helio_bridge`.)
- NOTE: during this phase the user's uncommitted submodule WIP referenced
  `pulsar_scenedb::handle_ledger`, which does not exist at the pinned rev
  `c761890` — when that work matures, either pin-bump SceneDB (workspace
  root, incl. the `[patch]` sections with the `//` URL spelling) or shim
  locally first.

### Deviations from issue text

- A5 said "fold world-transform into the GPU mirror" — done as a DERIVED
  COMPONENT (`ResolvedLightFrame`) instead, because the mirror type is
  auto-generated in the read-only submodule and cannot gain fields. The issue
  text explicitly allowed "(or a derived component)".
- A2's "delete `TickLoop.world`" is interpreted as "delete the owned private
  World"; the public surface keeps a world-shaped accessor path
  (`scene_store.write().world()`) because generated game projects depend on
  it — changing that API belongs to B's object-model contract.
- A3 landed as design + version gate + working host bridge + adopted-guest
  path; end-to-end dylib round-trip needs a running editor session (above).

### Verification status

At each commit: affected-crate `cargo test -p <crate>` green
(engine_backend 72, pulsar_game 39, pulsar_scene, pulsar_pie_abi),
`cargo check -p pulsar_engine` green, touched-file clippy clean. Final sweep
after `a0da33d8`: engine_backend 72/72, pulsar_game 39/39,
`cargo check -p pulsar_engine` OK, and a full `cargo check --workspace` OK.
CAVEAT: the user was concurrently editing the helio submodule AND their local
SceneDB tree (root `[patch]` temporarily repointed to `D:\GitHub\SceneDB`)
during this phase; workspace-wide results are only as stable as that WIP.
If something fails on your first run, check whether the failing symbol is one
of A's (`ResolvedLightFrame`, `ResolvedMeshFrame`, `RuntimeLevel`,
`helio_bridge`, `scene_store`, ABI v2 fields) before suspecting drift —
everything above is committed and self-consistent.
