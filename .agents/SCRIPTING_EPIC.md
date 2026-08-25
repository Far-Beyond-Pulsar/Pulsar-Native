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
5. **Verification before handoff (SCOPED — user directive, Aug 2026)**:
   verify only what you touch: `cargo check -p <crate>` / `cargo test -p
   <crate>` for each crate you modify, plus clippy on your files. Do NOT
   gate on full-workspace builds; vendored/plugin/submodule code failing
   for reasons outside your diff is out of scope — never fix compilers you
   didn't break. Cross-repo resolution (submodule pointer pushes, pin
   bumps, generated-template refreshes) happens ONCE at epic-end
   validation.
6. **Commits**: one commit per issue (`#NNN: imperative-mood summary`),
   on branch `scripting-epic`. Never push. Leave YOUR touched crates green.
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

## Handoff: B

Branch `scripting-epic`, one commit per issue, helio submodule pointer and
the user's in-flight `Cargo.toml`/`Cargo.lock` handle-ledger WIP excluded
from all of them (my lock/manifest lines were staged surgically; their WIP
hunks remain uncommitted exactly as found):

| Commit | Issue | Summary |
|---|---|---|
| `72940e41` | #640 | script object model — ActorRef/ComponentRef handles with per-access liveness |
| `de4c3106` | #641 | invalid-handle contract — debug misuse asserts, churn property tests, handle-semantics page |
| `1ad584b3` | #639 | StableId <-> Entity resolution — script references survive save/load |
| `f902cbae` | #642 | identity types ride reflection — registry shims, Reflectable refs, dyn-dispatch demo |

### The one crate everything downstream calls

**`crates/core/pulsar_script_object_model`** (workspace dep name
`pulsar_script_object_model`). C, D, E, F: import identity handling from
here — never re-derive Entity/StableId/class-name plumbing elsewhere.
Dependency direction: scenedb + reflection + world_registry only; NO editor,
renderer, or engine_backend deps (so game-project crates and VM guests can
link it). `engine_backend` depends on it (one-way) and implements its two
host traits for `WorldSceneStore`.

### Exact public API surface (signatures)

```rust
// ── refs.rs ── value types (Copy/Clone-friendly; ComponentRef is Clone+Eq+Hash)
pub struct ActorRef(pub Entity);
impl ActorRef {
    pub fn new(entity: Entity) -> Self;
    pub fn entity(self) -> Entity;
    pub fn is_alive(self, world: &World) -> bool;
    pub fn validate(self, world: &World) -> Result<(), ScriptRefError>;
    pub fn component(self, class_name: impl Into<String>, component_index: u32) -> ComponentRef;
    pub fn despawn(self, world: &mut World) -> bool;
}
impl From<Entity> for ActorRef {}

pub struct ComponentRef { pub entity: Entity, pub class_name: String, pub component_index: u32 }
impl ComponentRef {
    pub fn live(actor: ActorRef, class_name: impl Into<String>) -> Self;   // index-0 shorthand
    pub fn actor(&self) -> ActorRef;
    pub fn validate(&self, world: &World) -> Result<(), ScriptRefError>;   // liveness + registration
    pub fn is_valid(&self, world: &World) -> bool;

    // Property access (#640). JSON surface per issue text; live-typed path only.
    pub fn get_property(&self, world: &World, property: &str)
        -> Result<serde_json::Value, ScriptRefError>;
    pub fn set_property(&self, world: &mut World, property: &str, value: serde_json::Value)
        -> Result<(), ScriptRefError>;
    // Full panel-parity routing: non-live indexes go through store's records.
    pub fn get_property_with_instances(&self, world: &World,
        store: Option<&dyn ComponentInstanceStore>, property: &str)
        -> Result<serde_json::Value, ScriptRefError>;
    pub fn set_property_with_instances(&self, world: &mut World,
        store: Option<&mut dyn ComponentInstanceStore>, property: &str, value: serde_json::Value)
        -> Result<(), ScriptRefError>;
    // Method dispatch through MethodMetadata.caller (C1 supersedes internals,
    // NOT this signature). Always targets the live-typed value.
    pub fn call_method(&self, world: &mut World, method: &str, args: MethodArgs)
        -> Result<MethodReturnValue, ScriptRefError>;
}

// ── errors.rs ── the #641 taxonomy (thiserror Display, Clone+Eq)
pub enum ScriptRefError {
    ReferenceDespawned { entity_bits: u64 },      // dead/recycled/never-existed
    ComponentMissing { entity: Entity, class_name: String },
    ClassMismatch { expected: String, found: String, component_index: u32, entity: Entity },
    InstanceMissing { entity: Entity, class_name: String, component_index: u32 },
    UnregisteredClass(String),
    ClassNotBridged(String),                      // registration bug, surfaced not guessed
    UnknownProperty { class_name: String, property: String },
    UnknownMethod { class_name: String, method: String },
    Marshalling { context: String, message: String },
}

// ── instances.rs ── duplicate-instance storage seam (hosts implement)
pub struct InstanceRecord { pub class_name: String, pub enabled: bool, pub data: serde_json::Value }
pub trait ComponentInstanceStore {
    fn live_component_index(&self, entity: Entity, class_name: &str) -> Option<u32>;
    fn instance_record(&self, entity: Entity, index: u32) -> Option<InstanceRecord>;
    fn set_instance_data(&mut self, entity: Entity, index: u32, data: serde_json::Value) -> bool;
}

// ── subscribe.rs ── SceneDB#47 helpers
pub fn subscribe_component(world: &mut World, r: &ComponentRef) -> Option<SubscriptionId>;
pub fn take_change_events_for(world: &mut World, subscription: SubscriptionId)
    -> Vec<ComponentChangeEvent>;

// ── resolution.rs ── #639 save/load identity
pub trait StableIdResolver {                 // implemented for WorldSceneStore (engine_backend)
    fn entity_for_stable_id(&self, stable_id: &str) -> Option<Entity>;
    fn stable_id_for_entity(&self, entity: Entity) -> Option<String>;
    fn is_entity_alive(&self, entity: Entity) -> bool;
}
pub struct SerializedComponentRef            // THE serialized ref format ({stable_id,class_name,component_index})
    { pub stable_id: String, pub class_name: String, pub component_index: u32 }  // Serialize+Deserialize
pub enum ResolveRefError { ReferenceLost { stable_id: String }, Unidentified { entity: Entity } }
impl SerializedComponentRef { pub fn resolve<R: StableIdResolver + ?Sized>(&self, resolver: &R)
    -> Result<ComponentRef, ResolveRefError>; }
impl ComponentRef { pub fn to_serialized<R: StableIdResolver + ?Sized>(&self, resolver: &R)
    -> Result<SerializedComponentRef, ResolveRefError>; }

// ── reflect.rs ── #642 reflection integration
pub fn entity_type_info() -> &'static RuntimeTypeInfo;        // Primitive, color "#56D364"
pub fn actor_ref_type_info() -> &'static RuntimeTypeInfo;     // Wrapper(Custom "ActorRef"), "#F0883E"
pub fn component_ref_type_info() -> &'static RuntimeTypeInfo; // Struct{entity,class_name,component_index}, "#58A6FF"
impl Reflectable for ActorRef {}          // JSON: packed-bits number
impl Reflectable for ComponentRef {}      // JSON: {"entity":bits,"class_name":str,"component_index":n}
// Registry shims (hand-submitted RuntimeTypeRegistration, see upstream notes): Entity, u32,
// ActorRef, ComponentRef -- so serialize_json_for_any/deserialize_json_for_type work for all four.

// ── dispatch.rs ── #642 end-to-end dyn-dispatch demo (receiver = the World itself)
pub const RECEIVER_NAME: &str = "scene_object_model";
// DYN_METHOD_REGISTRY methods registered: "normalize_ref"(ComponentRef)->ComponentRef,
// "describe_ref"(ComponentRef)->String. Invalid/stale inputs => Ok(None)+warn, NEVER panic.

// ── contract.rs ── docs-only module: the handle-semantics page for script authors
```

### engine_backend additions (additive to handoff-A surface)

- New file `crates/core/engine_backend/src/scene/script_ref_bridge.rs`:
  `impl StableIdResolver for WorldSceneStore` and
  `impl ComponentInstanceStore for WorldSceneStore`. No new storage — the
  resolver reads the existing `by_stable_id` map / `StableId` components;
  the instance store reads/writes `RenderProps.component_instances` JSON
  with `pulsar_scene::component_instances_from_props`' exact index rule
  (explicit `"index"` field wins, else array position; missing `enabled`
  means enabled).
- Integration test `crates/core/engine_backend/tests/script_ref_survival.rs`
  (4 tests): the #639 acceptance — save → load → resolved ref writes the
  intended component and ONLY it; deleted target ⇒ typed `ReferenceLost`;
  reparenting between sessions doesn't disturb references; freezing a
  despawned target is typed.

### What landed per issue

**B1 / #640.** Value-type handles above, plus panel-parity routing
(`routing.rs`): the index IS the identity — routing picks live-vs-duplicate
storage BEFORE any presence check, so a stale index can never land an edit
elsewhere (#519 discipline). Live-typed reads/writes go through
`pulsar_world_registry`'s EngineClass bridges (no JSON on the hot path;
`Mut` guards fire subscription/GPU events exactly like panel edits);
duplicates hydrate into a throwaway World from THEIR OWN record, edit, and
serialize back into that same record (#561's nesting-correct mechanism).
Live writes also persist the full new shape back into the instance record
when a store is supplied (#561 Bug B parity). Acceptance test: two entities
with the same class, refs held across despawn, per-target writes observed
through subscriptions — no panics, correct targets.

**B3 / #641.** Typed-error taxonomy above; every accessor validates before
touching storage. Debug-build misuse assertion on exactly ONE input:
`Entity::DANGLING` reaching an accessor (sentinel crossing a boundary =
raw-id abuse). Ordinary staleness (despawned, recycled generation,
out-of-range slot after a world rebuild) NEVER asserts — those are expected
`Err`s. Property tests (`property_tests.rs`, deterministic xorshift, 3
seeds × 2000 ops): spawn/despawn/slot-reuse churn under held refs proves no
cross-object write ever occurs and retired refs report clean staleness
forever; plus a targeted recycled-slot test. `contract.rs` is the
script-author documentation page (boundary marshalling table included).
Audit note: VM opcodes (D) and generated code (E) don't exist yet — they
MUST route every component op through this crate's accessors/errors (that
is the downstream promise); PIE v2 raw-u64 crossing converts at glue and
treats first accessor validation as the trust boundary.

**B2 / #639.** Serialized reference format
`{stable_id, class_name, component_index}` (+ `ActorRef` equivalent not
needed yet — actors resolve via the same resolver directly). Resolution is
LAZY (per access against the current table), which is what makes reload/
undo-redo/reparenting survival automatic; deletion reports typed
`ReferenceLost`, never silent rebinding (exact-match-only fallback policy,
documented in `resolution.rs`). Editor UX affordance (human-readable pin
names) is F's consumption of `describe_ref` + stable ids — deferred there.

**B4 / #642.** Identity types ride reflection end-to-end: registry entries
keyed by TypeId for `Entity` + gap-filling `u32`; full manual `Reflectable`
impls for `ActorRef`/`ComponentRef` (struct shape with real FieldInfo
offsets via `offset_of!`); distinct declared pin colors so F can color/
filter object-reference pins as their own type. Demo wired through the REAL
global `DYN_METHOD_REGISTRY`: receiver `"scene_object_model"` (the receiver
is the `World` itself — `World: Any+Send+Sync` slots into `&mut dyn Any`),
methods take a `ComponentRef` arg and return one back, refusing stale
inputs with warn+None instead of panicking. Marshalling rules table lives
in `reflect.rs`'s module doc and `contract.rs`.

### Shimmed vs upstream-blocked (upstream asks recorded, nothing edited)

- **`Reflectable for pulsar_scenedb::Entity`**: orphan rule forbids it
  (foreign trait × foreign type). Landed as a hand-written
  `RuntimeTypeRegistration` inventory submit keyed by `TypeId::of::<Entity>()`
  — the entire registry/marshalling path works off those fn pointers, so
  end-to-end JSON round-trip holds without the trait impl. UPSTREAM ASK
  (SceneDB): move the registration upstream (or into reflection's prims) so
  Entity gains the real trait impl, and consider `{slot, generation}`
  structure instead of opaque bits if pins ever display generations.
- **`u32`** has no entry in reflection's prim set (only i32/i64/u64...):
  same shim treatment locally; upstream ask to add it to prims.
- **`#[pulsar_type]` alias trick does NOT work cross-crate**: it generates
  `impl ForeignTrait for ForeignType` (E0117 outside pulsar_reflection),
  which is why the shims are hand-written submissions instead.
- **DEADLOCK TRAP discovered** (documented in reflect.rs): the runtime type
  registry calls every submitted `type_info()` fn during ITS OWN Lazy init,
  so descriptor construction must never touch `RUNTIME_TYPE_REGISTRY`
  (re-entrant access hangs forever). Our descriptors are plain statics; a
  local unregistered `String` field descriptor routes by TypeId through
  upstream's entry. Whoever adds more identity registrations must keep this
  property.

### Deviations from issue text

- `class_name: SmolStr` → `String`: the workspace has no smol_str dep and
  every adjacent identity type (`StableId`, `Name`) is plain `String`.
- Issue B1's `get_property(name) -> Result<Value>` is the live-typed pair
  (`get_property`/`set_property`); duplicate routing needs an instance
  store, so the routed variants are spelled `*_with_instances` (bare `None`
  for `Option<&mut dyn Trait>` doesn't infer — this keeps everyday call
  sites clean).
- B4's "wire one #[method]" landed as DYN_METHOD_REGISTRY methods rather
  than an EngineClass `#[method]`: component-method metadata callers take
  `&mut dyn EngineClass` receivers with NO world context, so a method
  needing liveness validation can't be expressed there today. The dyn
  registry's `&mut dyn Any` receiver accepts the World itself. C1 may want
  to note that context-injection gap when building the unified dispatcher.
- B2's editor UX scope item (human-readable pickers) belongs to F; the data
  it needs (stable ids + Name components + `describe_ref`) exists.

### Verification status

At each commit: affected-crate tests green (object model 38/38 incl. 5-seed
churn property tests; engine_backend lib 77/77 incl. 5 new bridge unit
tests + 4/4 new integration tests), touched-file clippy clean. Final sweep:
`cargo check --workspace` OK. Pre-existing failures untouched (two
toggle_button.rs doctests; workspace clippy warnings on unrelated files;
SceneDB-local warnings from the user's local-tree override).

CAVEAT (same as A's): the user's `[patch]` still points
`pulsar_scenedb` at their LOCAL tree (`D:\GitHub\SceneDB`, handle_ledger
WIP — purely additive diff vs pinned `c761890`; I verified every API I use
exists identically at the pin). Workspace-wide results are only as stable
as that override; my code compiles against BOTH the pinned rev's API and
the local tree.

### Notes for C/D/E/F

1. **Funnel through this crate.** C's `invoke_component_method` should
   reuse `ComponentRef::{validate, call_method}`'s semantics (and ideally
   delegate to them) rather than re-implementing dispatch against raw
   `(entity, ComponentId)` pairs; D's `comp_*` opcodes and E's generated
   code must return `ScriptRefError` variants across their boundaries.
2. **Marshalling (C2):** identity values cross reflection boundaries boxed
   as the concrete types (`Box::new(component_ref)`); cross FFI as
   `Entity::bits()` u64 only. The shims make both directions registry-
   serializable already.
3. **Pins (F):** `actor_ref_type_info()`/`component_ref_type_info()` carry
   declared colors; `describe_ref` produces the human-readable label.
   Persisted pins serialize `SerializedComponentRef`.
4. **Locking:** accessors are lock-scope-agnostic — pass `store.read().world()`
   / `.write().world_mut()` per call, keep write scopes short (A's contract
   unchanged). Never hold a store guard across script callbacks that might
   re-enter the store.

## Handoff: C

Branch `scripting-epic`, one commit per issue, helio submodule pointer and
the user's `Cargo.toml`/`Cargo.lock` handle-ledger WIP excluded from all of
them (the ONE lock line C1 required — `thiserror` added to
`pulsar_world_registry`'s dep list — was staged surgically via
`git apply --cached`; their WIP hunks remain uncommitted exactly as found):

| Commit | Issue | Summary |
|---|---|---|
| `b7d5f711` | #643 | unified reflection dispatcher -- invoke_component_method + property accessors over live World components |
| `95d8a72a` | #644 | unified marshalling -- JSON/Any/arena-bytes conversions with versioned VM TypeSlot encoding spec |
| `25d9d044` | #645 | reflection metadata audit -- overload policy, purity requirement, title-cased display names, golden registry snapshot |

### The keystone: exact public API (D and E call THESE)

All in `pulsar_world_registry` (`crates/core/pulsar_world_registry`).
Dependency direction unchanged: scenedb + reflection + serde_json +
inventory + thiserror. `pulsar_script_object_model` still depends on it
(one-way), so game crates and VM guests get everything transitively.

```rust
// ── dispatch.rs (#643) ──────────────────────────────────────────────────
pub fn invoke_component_method(
    world: &mut World,
    entity: pulsar_scenedb::Entity,
    class_name: &str,
    component_index: u32,
    method: &str,
    args: pulsar_reflection::MethodArgs,            // Vec<Box<dyn Any>>
) -> Result<pulsar_reflection::MethodReturnValue,   // Option<Box<dyn Any>>
            ScriptRefError>;

pub fn get_component_property(world: &World, entity: Entity,
    class_name: &str, component_index: u32, property: &str)
    -> Result<serde_json::Value, ScriptRefError>;   // editor/metadata path

pub fn set_component_property(world: &mut World, entity: Entity,
    class_name: &str, component_index: u32, property: &str,
    value: serde_json::Value) -> Result<(), ScriptRefError>;

pub fn get_component_property_boxed(..same ids..)   // NO-JSON hot path (#D4)
    -> Result<Box<dyn Any>, ScriptRefError>;
pub fn set_component_property_boxed(..same ids.., value: Box<dyn Any>)
    -> Result<(), ScriptRefError>;
```

Resolution order in `invoke_component_method` (every step a typed error,
NEVER a panic): liveness → `UnregisteredClass` → `UnknownMethod` →
argument arity/`TypeId` validation (**performed here** because the derive-
generated caller closures panic on missing/wrong args; the dispatcher
refuses first with `ArgumentCount`/`ArgumentType`) → presence of the live-
typed value (`ComponentMissing`) → `caller(args)` on `&mut dyn EngineClass`
through `get_world_component_as_engine_class_mut`. Mutations ride the real
World storage, so SceneDB `Mut` guards fire subscription/GPU events exactly
like panel edits.

Index semantics (mirrors B exactly): PROPERTIES at this layer address only
index 0 (live-typed); other indexes are `InstanceMissing` — duplicate
records stay behind the object-model crate's `ComponentInstanceStore`.
METHODS execute against the live-typed value regardless of index (class-
level behavior; B's `ComponentRef::call_method` semantics, which now
DELEGATES to this dispatcher — one dispatch path total).

```rust
// ── errors.rs — THE taxonomy, moved + extended (#643) ───────────────────
// Canonical home is now pulsar_world_registry::errors (next to the
// dispatcher). pulsar_script_object_model::errors re-exports it, so every
// pre-existing path is byte-identical. New variants for #643:
pub enum ScriptRefError {
    // ...all #641 variants unchanged (ReferenceDespawned, ComponentMissing,
    // ClassMismatch, InstanceMissing, UnregisteredClass, ClassNotBridged,
    // UnknownProperty, UnknownMethod, Marshalling)...
    ArgumentCount { class_name: String, method: String,
                    expected: usize, got: usize },
    ArgumentType  { class_name: String, method: String, index: usize,
                    param: &'static str, expected: &'static str,
                    found: String },   // found = registered type_name or raw TypeId debug
}
impl ScriptRefError { pub fn despawned(entity: Entity) -> Self }  // now pub (cross-crate)
```

```rust
// ── marshal.rs (#644) ───────────────────────────────────────────────────
pub fn any_to_json(context: &str, value: &dyn Any) -> Result<Value, ScriptRefError>;
pub fn json_to_any(context: &str, type_info: &'static RuntimeTypeInfo,
                   value: Value) -> Result<Box<dyn Any>, ScriptRefError>;
pub fn any_to_bytes(type_info: &'static RuntimeTypeInfo, value: &dyn Any,
                    out: &mut Vec<u8>) -> Result<(), ScriptRefError>;
pub fn bytes_to_any(type_info: &'static RuntimeTypeInfo, bytes: &[u8])
    -> Result<Box<dyn Any>, ScriptRefError>;
```

Exactness invariant both directions: the box holds EXACTLY
`type_info.type_id`'s type or it's an `Err`. All failures are
`ScriptRefError::Marshalling { context, message }`.

### TypeSlot encoding spec (Phase D consumes directly)

**Location: `pulsar_world_registry/src/vm_abi.rs`** — module doc IS the
spec; `TYPE_SLOT_ENCODING_VERSION = 1`. Summary:

- `VmTypeSlot` (repr(C), 24 bytes): `{ size: u64, align: u64, kind: u32,
  reserved: u32 (=0) }`. First 16 bytes ARE a `pulsar_std::TypeSlot`
  (legacy-prefix compatible; `slot_for(info)` / `.legacy_prefix()` build
  the bridge). Readers must refuse unknown kinds/non-zero reserved.
- Per-kind value layouts (NATIVE endian — in-process calling convention,
  NOT portable serialization): `Direct=0` inline native bytes (numeric
  prims ≤ 8B, bool as 1 byte, Entity as packed bits-u64);
  `Utf8String=1` `[u64 len][utf8]`; `Vector=2` `[u64 count][count × Direct
  elems]`; `JsonEncoded=3` `[u64 len][utf8 JSON]` — universal fallback for
  every REGISTERED type. `classify(type_info)` is the single decision both
  encode/decode drive from.
- Fast paths are the ONLY hardcoded type lists (issue-compliant); the
  closed Direct set lives in `marshal::is_direct_type`. Everything else
  routes by classification; unregistered types are refused, never guessed.
- Performance note for #D4: hot sets compose
  `set_component_property_boxed` + `any_to_bytes`/`bytes_to_any` — no JSON
  for Direct/String/Vector kinds. JSON is editor/metadata-only.
- `type_shims.rs`: upstream registers Vec<T>/Option<T> LAZILY but never in
  the inventory registry, so registry-level JSON legs refused them; shimmed
  locally (Vec<f32/f64/i32/u64/String>, Option<bool/i32/f32/String>) using
  upstream's own Reflectable impls. Add instantiations there as components
  demand them.

### Audit results (#645)

Sweep coverage: every `#[engine_class]` class linked in Pulsar-Native-proper
— `RigidbodyComponent`, `PhysicsComponent` (+ their `#[sub_props]` groups),
engine_class_derive test fixtures, object-model TestGizmo — plus the
helio-component classes READ-ONLY (same generator, same conclusions).
Findings:

1. **Purity**: correct today by construction — the only generated methods
   are property accessors (getters `Pure`, setters `Fn`); zero hand-written
   `#[method]`s existed workspace-wide. Landmine removed: `#[method]` now
   REQUIRES explicit `type = Pure|Fn|ControlFlow` (compile error otherwise;
   silent default was Pure, and rust_codegen inlines Pure bodies).
2. **Overload policy**: DISALLOWED, two layers. Compile-time:
   duplicate `#[method]` names within one impl block are a compile error
   (name-keyed `REGISTRY.get_method` would shadow). Link/test-time:
   `pulsar_world_registry::find_overloaded_methods() -> Vec<MetadataAuditError>`
   sweeps all classes across registrations; asserted empty in CI.
3. **Display names/categories**: all generated display names now title-case
   each word ("Linear Damping", "Add Charges"); auto get_/set_ accessor
   methods inherit their FIELD's category instead of landing ungrouped.
   No code string-matches display names (verified by grep) — labels are
   render-safe to change.
4. **Golden snapshot**: `pulsar_physics/src/golden_metadata.rs` (unit-test
   module ON PURPOSE — integration binaries linker-GC arbitrary inventory
   statics; observed classes-without-methods) diffs
   `metadata_snapshot_json()` against
   `tests/expected_registry_snapshot.json` (77 properties captured, fully
   sorted/deterministic). Regen: `PULSAR_UPDATE_SNAPSHOT=1 cargo test -p
   pulsar_physics golden_metadata`. Reusable everywhere: any crate linking
   real classes can drop the same two tests in.
5. **Gap recorded, not fixed**: top-level components whose fields are ALL
   `#[sub_props]` (both physics components) expose ZERO reflected METHODS —
   accessors generate per direct `#[property]` field only. Properties
   remain fully reachable via nesting + C1's property API, so scripts lose
   nothing today; generating nested accessors is macro surgery deferred
   until a consumer demands it.
6. **Doc-comment→tooltip**: `MethodMetadata` has no docs field (upstream
   type) — follow-up filed under upstream asks below.

### Upstream asks (recorded, nothing edited; pulsar_reflection @ 745ee78)

1. **Reflectable derive bug (blocks derived structs with String/Vec)**:
   generated `deserialize` does `*value.downcast_ref::<FieldTy>()?` — a
   MOVE out of a shared reference; fails E0507 for ANY non-Copy field.
   Should `.clone()` (fields already bound `Clone`). Worked around via
   hand-written impl in marshal tests; helio/engine structs wanting
   `#[derive(Reflectable)]` with strings will hit this.
2. Register common wrapper instantiations (`Vec<T>`, `Option<T>`) in the
   inventory registry (or add a lazy fallback) so `type_shims.rs` can
   shrink to nothing — same protocol as B's Entity/u32 asks.
3. `MethodMetadata`/`PropertyMetadata`: consider a docs/tooltip string
   (doc-comment capture needs derive support too) — feeds #645's tooltip
   pipeline item for F.
4. `capitalize_first`'s single-word output was replaced by `title_case`
   inside engine_class_derive (Pulsar-Native-side); no upstream change
   needed — listed only because helio-component's GENERATED labels change
   with the next recompile (no submodule edit involved).

### Deviations from issue text

- #643 named `InvokeError`; the shared taxonomy WON instead — dispatcher
  returns `ScriptRefError` directly (guidance: reuse, don't fork). The
  issue's `ArgumentMismatch` landed as two precise variants
  (`ArgumentCount`/`ArgumentType`) rather than one message-shaped enum.
- #643's "validate index (live-typed vs duplicate routing)" resolves at
  THIS layer as: properties reject non-zero indexes (`InstanceMissing`),
  methods ignore index (documented class-level semantics, mirroring B).
  Duplicate-aware METHOD routing stays impossible here because instance
  records live behind the object-model store seam, not world_registry.
- #644's "arena bytes" for compounds is length-prefixed staging owned by
  the caller (spec'd in vm_abi) rather than fixed-size slots — primitives
  are the zero-copy inline case; strings/vecs/structs cannot be honestly
  zero-copy into a slot ABI without ownership hazards. Documented as
  allocation cases per the issue.
- Golden test lives in `src/golden_metadata.rs` (cfg(test)) instead of
  `tests/` — see linkage note above; determinism beats convention here.

### Verification status

Per commit: affected-crate tests green (final sweep: world_registry 23 lib
+ 10 integration, object_model 36, engine_class_derive 21 incl. its own
suites, physics 2 golden + existing suites untouched-green), touched-file
clippy clean (remaining warnings are SceneDB local-tree + pre-existing
physics import warnings, none in touched files),
`cargo check --workspace` OK. Same CAVEAT as A/B: user's `[patch]` points
scenedb at their local tree; everything above compiles against BOTH that
and the pinned rev's APIs I touch.

### Notes for D/E/F

1. Call `invoke_component_method` / `*_component_property_boxed` — never
   `REGISTRY.get_method` + caller yourself (you'd re-introduce the panic
   paths the dispatcher guards).
2. VM glue: read `vm_abi.rs` FIRST; encode args with `any_to_bytes`,
   decode returns with `bytes_to_any`, build slots with `slot_for`.
   Version-gate on `TYPE_SLOT_ENCODING_VERSION`.
3. Errors crossing your boundaries are `ScriptRefError` (re-exported from
   `pulsar_script_object_model`, canonical in `pulsar_world_registry`) —
   keep matching on variants, never on Display strings.
4. F: palette grouping now gets categories on accessor nodes + stable
   title-cased labels; golden snapshots exist for physics and are cheap to
   clone for helio-side classes when they become editable.

## Handoff: D (in progress)

### D1 — component-op bytecode ABI (#646) — LANDED

Commits: pbgc `e2fe3c1` (submodule-internal), parent `09576490`
(CompiledBytecode v2). NOTE: `crates/third-party/pbgc` is a SUBMODULE —
its changes are committed inside it; the user must push/bump pointers
(same protocol as plugins/vendor/*).

Encoding (spec module: `pbgc/src/bytecode/comp_ops.rs` — module doc IS
the contract):

- All three kinds compile to plain `Instruction::Call`; routing key is
  the full `comp_*::Class::Member` string retained in `node_type`.
- `input_offsets[0]` = name blob `{class}\0{member}\0` (dedup per event).
  Values = JSON blobs `[u64 LE len][utf8]`. Constants stage exact bytes;
  runtime outputs reserve `JSON_BLOB_CAPACITY` (4 KiB).
- GetProp is emitted from BOTH exec chains (defensive) and the pure
  dependency path (`ensure_connected_output`) — it produces arena values
  like any pure node.
- Native-source value inputs are a COMPILE ERROR for now ("#647 adds
  live conversion"). Constants, defaults (JSON null), and get→set/call
  chains work.
- `compile_graph_to_bytecode_full() -> BytecodeCompilation { programs,
  components }` reports deduplicated ComponentOpRefs; old fns delegate.
- CompiledBytecode / editor BytecodeFileOutput: version bumped to 2,
  new `components: Vec<pbgc::ComponentOpRef>` field with serde default
  (v1 files deserialize fine).

Executor hook surface for D2 (what the stubs proved): patch comp_-
prefixed Calls' fn_ptr to handlers with signature DispatchFn; handler
reads name blob by scanning two NULs; get writes its JSON into `ret`
(reserved capacity); call's ret is null when void.

Tests: 27 integration in `pbgc/tests/bytecode_tests.rs` (encoding
round-trips, chaining, output elision, serde, stub execution of all
three kinds incl. cross-instruction value flow), plus CompiledBytecode
v2 assertions in pulsar_game. Full pbgc suite green.

### D2 — VM execution context (#647) — LANDED

Commits: pbgc `fccd2d9` (submodule: call blobs stage argc), parent
`37a7a9b0`.

- `pulsar_bp_executor`: `ComponentOpHandlers { get, set, call }` +
  `prepare_with_component_ops(program, handlers)`. comp_-prefixed calls
  bypass whitelist AND dlsym; plain `prepare()` now returns new error
  variant `ComponentOpsNotBound { node_type }` for them.
- `pulsar_game::blueprint_runtime::component_ops` (NEW module, ~320
  lines): thread-local `{*mut World, Option<Entity>}` context installed
  by `run_with_component_context(world, entity, f)` (RAII-cleared,
  nested install panics). Trampolines parse staged operands and route
  through `pulsar_world_registry::dispatch::*` — ONE dispatch path.
  comp_call converts JSON args to declared types via C2's
  `marshal::json_to_any` before dispatch (strict ArgumentCount/Type
  validation preserved). All failures LOG + degrade to null; no unwind
  across extern "C". Error surfacing into graph results = #648 scope.
- Call name blobs now stage argc (`{class}\0{method}\0{n}\0`,
  pbgc `encode_call_name_blob`) — the handler needs operand count with
  no out-of-band length.
- Executor: `execute_event_in_world(class, event, arena,
  Option<(&mut World, Entity)>)`; legacy `execute_event` delegates with
  None. Dispatcher dispatch methods take `&mut World`; TickLoop phase 3
  + shutdown now hold the store write lock while dispatching.
- Handlers obtained via `component_op_handlers()` fn — fn-ptr-to-int
  casts are rejected in const eval.

TESTS (pulsar_game lib, 4 passing): set writes live property; get reads
live value back out of reserved blob; comp_call dispatches through
reflection with typed args and return; no-context runs refuse without
panic. Workspace check green; clippy clean on touched files.

D3 entry point: dispatcher currently passes `Entity::DANGLING` for all
instances — replace with real per-entity binding + lifecycle events.

### D4 — value types through the VM (#649) — CORE LANDED

Commits: `df8081f3` (world_registry marshal helpers), `fa900ee2`
(instance override marshalling). 46/46 pulsar_game lib tests.

- Variable overrides: primitives keep LE fast paths; String / Vec<T> /
  registered structs resolve via new
  `marshal::{serialize_named_json, deserialize_named_bytes}` and emit
  the VM blob encoding. Unknown names = typed errors (never zeroed).
- Alias resolution added to marshal: editors say `Vec<f32>` /
  `Option<i32>`, wrappers register as `alloc::vec::…` / `core::option::…`
  — both directions probed empirically and covered.
- KNOWN LIMIT (design note for later): in-graph heap-typed VARIABLES
  (StoreVar/LoadVar of a real Rust String) would need ownership-aware
  arena slots; today compound values flow through component ops' JSON
  domain (D1/D2) which covers graph-level usage. Editor make/break-
  struct nodes do not exist yet — node-library work, belongs with F.

### D3 — dispatcher ↔ world integration (#648) — LANDED

Commit: one parent-repo commit (branch `scripting-epic`), pulsar_game only.
52/52 lib tests.

- **Binding model**: `BlueprintInstance.entity: Option<Entity>` (private +
  accessors). Instances start unbound; `BlueprintDispatcher::
  spawn_instance_for_entity(id, path, entity, overrides)` registers AND
  binds atomically (gameplay-driven spawning), `bind_instance`/
  `unbind_instance`/`instance_entity` cover late binding and respawn
  rebinding. Multiple instances of one class coexist — map is keyed by
  object id, each entry owns arena + binding. `register_bytecode` is the
  in-memory core all registration funnels through, and it SHORT-CIRCUITS
  class preparation when the class is already loaded (spawning N enemies
  prepares once; deliberate swaps go through reload).
- **Dispatch**: `execute_event_in_world` now takes
  `Option<EventWorld>` (`EventWorld { world: &mut World,
  entity: Option<Entity> }`, ctors `bound`/`unbound`) instead of the old
  `Option<(&mut World, Entity)>`; the DANGLING-entity hack is deleted.
  Unbound instances run graphs with component ops refusing (accurate log),
  bound instances address their own entity. New bulk
  `dispatch_tick_all(world, delta)` replaces the TickLoop's per-frame
  key-snapshot loop (phase-3 write lock unchanged). New typed error
  `ExecutorError::InstanceNotRegistered`.
- **Lifecycle**: begin_play queue unchanged (drains on first tick);
  tick + end_play dispatch per instance against the shared world.
  Despawned entities are NOT auto-unbound — component ops refuse via
  liveness checks per event (tested: sibling keeps ticking).
- **Hot reload wired at runtime level**: `BlueprintDispatcher::
  reload_blueprint(bytecode)` = executor program swap PLUS
  `BlueprintInstance::rehydrate_after_reload` for every affected instance —
  fresh arena from the new layout, carrying over ONLY variables whose
  `(name, data_type, offset, size)` match exactly (sentinel-tested: drifted
  layouts can never alias old bytes). This IS the PIE-recompile entry the
  editor must call; wiring it into the vendored blueprint_editor's recompile
  flow is #650/editor work (plugin changes were out of scope here).
- **Arena sizing fix (latent bug found)**: instance arenas were sized from
  variable extent only; any program whose scratch exceeds that failed with
  VM `InsufficientArena` AT RUNTIME. `required_arena_size` now takes
  max(variables, every event program's `arena_size`) at creation AND on
  reload. Real editor output with comp-op JSON blobs (4 KiB reservations)
  would have hit this on first real graph.
- **Acceptance test** (`tests::blueprint_instances`): two entities sharing
  one class, full TickLoop phase-3 path — independent charges (7 vs 52),
  per-entity begin_play, distinct arenas (override vs default); plus
  despawn-isolation, late-bind, and hot-reload preservation tests.

NOT DONE HERE (by scope): level.json object→class bindings (#650 calls
`bind_instance`/`spawn_instance_for_entity` at load), self-reference `self`
ActorRef pin convention (needs pbgc node/variable work with F — the bound
entity is available to hosts via `instance_entity`, so a `self` variable can
be injected by whoever wires node support), delta-time into programs
(`execute_tick` reserves the parameter).

Notes for E/F: `EventWorld` replaces tuple contexts in any host code you
write; `dispatch_tick_all`/`dispatch_pending_begin_play` under a store write
lock is THE lifecycle contract; `instance_variable_bytes` is the tooling seam
for inspectors. Pre-existing crate clippy blockers (untouched files):
`bytecode_compiler.rs` never_loop (deny) + approx_constant — first person
touching that file should fix both.

REMAINING IN PHASE D: #650 (level.json bindings format + editor UX),
then phases E (#651-653) and F (#654-656). Subagent provider was
returning "Endpoint is unavailable" for long implementer sessions
during this phase — A/B/C ran fine earlier; retry subagents before
assuming inline work is required.

### D5 — level-format blueprint class bindings (#650) — LANDED

One parent-repo commit `76dc0fdf` (branch `scripting-epic`). Touched crates:
pulsar_scene, engine_backend, pulsar_game, ui_level_editor (format round-trip
preservation only). The vendored blueprint_editor was NOT touched — it had
uncommitted user WIP in `src/features/compilation/compiler.rs`; see Editor
UX below for what F builds.

- **Format** (`pulsar_scene::format`): new additive top-level section on
  `SceneFile`: `blueprint_bindings: BlueprintBindings` (= `BTreeMap<String,
  Vec<BlueprintBinding>>`, object **StableId** → class bindings).
  `BlueprintBinding { class_name, overrides: HashMap<String, Value> }`.
  BTreeMap so load order is deterministic; `skip_serializing_if` empty so
  pre-#650 writers stay byte-compatible; old files deserialize empty
  (serde default). Multiple classes per object = multiple vec entries.
  Overrides are variable-name → natural-JSON-value, consumed by D3/D4's
  existing override marshalling unchanged.
- **Loader** (`pulsar_game::blueprint_runtime::level_bindings`, NEW module):
  `apply_blueprint_bindings(dispatcher, &store, project_root, &bindings) ->
  ApplyReport` resolves each StableId via the hydrated store, locates
  bytecode at `<root>/src/classes/<class>/events/.build/bytecode.json`
  (`bytecode_path_for_class` — byte-identical to generated engine_main
  discovery), and spawns through `BlueprintDispatcher::
  spawn_instance_for_entity`. Instance ids are deterministic:
  `{stable_id}::{class_name}` (`instance_id_for`). Per-binding failures are
  typed (`BindingError::{UnknownObject, DuplicateClass, BytecodeMissing,
  Io, Serialization, Executor}`), collected + logged — one stale entry never
  blocks play mode. Runtime add/remove for gameplay-driven scripted objects:
  `bind_object_class` / `unbind_object_class`.
  Play-mode wiring: `PulsarApp::open_window_with_scene` applies the level's
  bindings right after hydration (dispatcher mutex THEN store write — same
  lock order as TickLoop phase 3).
- **API change (engine_backend)**: `RuntimeLevel::load_into` now returns
  `Result<LevelExtras, _>` where `LevelExtras { editor_camera,
  blueprint_bindings }` (was bare `Option<EditorCamera>`); owned-store paths
  expose `.extras()`. Sole caller updated. Bindings deliberately do NOT
  auto-spawn inside hydration — dispatcher ownership stays gameplay-side.
- **Editor save-path preservation** (`ui_level_editor::scene_database`):
  its parallel `LevelFile` type gained the same-typed field, preserved on
  save by reading the existing file back (mirrors `preserved_editor`) — an
  ordinary editor re-save can no longer destroy authored bindings even
  though no binding UI exists yet.
- **Migration decision**: helio's `ScriptComponent`/`SCRIPT_REGISTRY` path is
  documented as superseded-but-functional (module doc of level_bindings.rs).
  NOT auto-converted: ScriptComponent stores a blueprint *directory*
  (`graph_save.json`), not a compiled class name — directory→class mapping
  is editor policy, so conversion belongs to F's inspector UX (offer
  binding authoring when a ScriptComponent is present).

Schema example (committed fixture, used by the acceptance test):
`crates/core/pulsar_game/tests/fixtures/level_bindings_sample.level.json`
— two objects (`lever_a`, `lever_b`) bound to ONE class (`TickProbe`) with
DIFFERENT overrides (`speed: 2.5` vs `9.0`).

Acceptance tests (pulsar_game lib): fixture parses + hydrates → two
instances on distinct entities, independent charges through real dispatch,
distinct override bytes per arena; removing a binding unregisters cleanly
and the sibling keeps ticking; duplicate/stale bindings fail individually
never fatally. Plus format round-trip/additivity tests (pulsar_scene),
extras tests (engine_backend), and editor save/load preservation tests
(ui_level_editor).

For F (editor UX checklist):
1. Inspector/outliner assign-unassign UI writes `SceneFile.blueprint_
   bindings` through the editor save path (field already round-trips);
   key by `obj.id` (StableId), one vec entry per class.
2. Override editing: read variable names/types from compiled bytecode
   (`CompiledBytecode.variables`); write JSON values into `overrides`;
   `instance_variable_bytes` / `rehydrate_after_reload` are the live-
   debugger seams.
3. PIE gap (deliberate): the ABI-v2 guest skips level hydration entirely,
   so PIE does not spawn level-bound instances yet — the HOST side must
   apply extras' bindings after its hydrate (same function, needs the
   guest's dispatcher handle wired host-side first).
4. When a ScriptComponent exists on an object, offer "convert to level
   binding" (resolve directory → class name) rather than silent dual paths.

Surprises/deviations: (a) the editor saves levels through its OWN LevelFile
type, not pulsar_scene's SceneFile — any future additive section MUST land
in both or be silently dropped on save; handled here, worth remembering.
(b) `load_into` signature change was unavoidable (bindings must reach the
host without re-parsing the file). (c) ByteArena panics on size 0 — empty
test bytecode needs at least one variable.

Verification status: scoped per quality bar rule 5 — pulsar_scene 4/4,
engine_backend lib 78/78, pulsar_game lib 59/59 (7 new), ui_level_editor
lib 46/46 (2 new); clippy clean on every touched file (pre-existing
pulsar_game clippy deny in bytecode_compiler.rs untouched, see D3 note;
wgpui/gpui-ce warnings unrelated). Cargo.toml/Cargo.lock user WIP excluded;
only MY single `ui_level_editor → pulsar_scene` lock line staged (surgical
`git apply --cached`, same protocol as C/D3).

## Handoff: E (in progress)

### Handoff: E1

Branch `scripting-epic`, commit `4518f212` (parent repo) + pbgc submodule
commit `c4aefa2` (inside `crates/third-party/pbgc`, on top of E2's
`bc9929d`; parent gitlink deliberately NOT bumped — same protocol as D/E2,
the user pushes/bumps pointers). Issue #651. STATUS: COMPLETE.

**What landed**

1. **Baked-store routing deleted outright** (no flag, per issue preference).
   Generated actors are now ALWAYS `pub struct {Ty} {}` — no
   `Arc<pulsar_game::ComponentStore>` field, no `__bp_set_comp_ctx`/
   `__bp_clear_comp_ctx` around logic, no `gamma_core` remnants. pbgc's
   `rust_codegen.rs` comp_* emission routes through C's dispatcher against
   the `(entity, world)` pair every Actor callback receives:

   ```
   BEFORE:  pulsar_game::__bp_with_comp(|__store| { __store.set_property_json(
                "Light", "intensity", serde_json::to_value(X).unwrap_or(..)); });
            // private Arc<ComponentStore> hydrated from compile-time JSON;
            // Actor impl ignored its (_entity, _world); logic fns took ().

   AFTER:   if let Err(__e) = pulsar_world_registry::dispatch::set_component_property(
                _world, _entity, "Light", 0, "intensity",
                serde_json::to_value(X).unwrap_or(serde_json::Value::Null)) {
                tracing::error!("comp_set_prop::Light::intensity failed: {__e}");
            }
            // comp_get_prop -> dispatch::get_component_property(..).unwrap_or(Null)
            // comp_call     -> dispatch::json_args_to_method_args(..) then
            //                  dispatch::invoke_component_method(..), return
            //                  marshalled back to JSON via marshal::any_to_json
   ```

   One dispatch layer, two adapters: the VM trampolines and generated Rust
   now share `dispatch::json_args_to_method_args` (NEW in world_registry)
   for the JSON→declared-type conversion — component_ops.rs' inline copy was
   deleted in favor of it. Failures log-and-degrade-to-null identically on
   both paths.

2. **Logic functions carry the live world.** Every generated event fn
   (`begin_play`/`tick`/`on_*`) ends with
   `_entity: pulsar_game::Entity, _world: &mut pulsar_game::World` (fully
   qualified so vars/pulsar_std globs can never shadow), and the template
   forwards `_entity, _world` at each call site. The Actor-callback
   signatures themselves are UNCHANGED (E2's pinned-trait guards still pass
   byte-identical).

3. **Prefab components hydrate REAL entities, absent-only.**
   `{Ty}::__init_components(entity, world)` (now a self-free assoc fn) gates
   each enabled prefab component on NEW
   `pulsar_world_registry::world_component_present_for_class`, hydrating
   baked defaults only when the scene hasn't already provided the component
   — per-instance scene values always win (#651 scope item 3).
   `__run_component_begin_plays` re-routes through
   `dispatch::invoke_component_method` (UnknownMethod = class has no
   begin_play = silent skip).

4. **core_project_builder**: spawn/registration flow itself needed NO
   functional change (post-A2 it already registers into the shared store;
   hydration happens inside begin_play with the register-supplied world) —
   the emitted spawn statement gained a comment documenting the live-world
   contract. `LevelPrefabEntry.variable_overrides` remains parsed-but-unused
   for NATIVE actors (they have no instance variables; pre-existing).

5. **pulsar_game lib surface**: the SceneDB baked-store helpers
   (`__bp_with_comp`, `__bp_set_comp_ctx/clear`, `ComponentStore`) are no
   longer re-exported from `pulsar_game`/its prelude (still available from
   `pulsar_scenedb` itself; grep-verified zero remaining consumers in-tree,
   vendored plugins clean).

6. **Template-Blank refreshed** (`src/classes/MyBP/events/events.rs` ONLY)
   through the real pipeline (graph → compile_graph →
   generate_blueprint_actor_source_with_components with the prefab.json
   LightComponent) via a throwaway cargo script in %TEMP% — post-E2/#651
   shape: time-free tick, empty struct, dispatcher routing. The old file's
   inert custom-event stubs (`#[pulsar_event] struct D0f87…`,
   `on_d0f876aa…()`) are gone — they came from a macro/custom-event node
   that is never called by anything; the live begin_play→branch(false)→
   println chain is preserved byte-equivalently. NOTHING ELSE in that repo
   touched (it is full of user WIP); NOT committed there (no protocol for
   that repo; user reviews alongside their WIP). engine_main.rs there was
   already current-shape.

**Acceptance evidence (#651 criterion)**

Full light e2e needs a GPU/display session (same caveat as A5/#635), so per
the issue's sanctioned fallback, the registered-probe pattern proves
live-world mutation through GENERATED-SHAPE code:
`pulsar_game::blueprint_live_dispatch` (NEW lib-test module):
- `LiveDispatchReference` hand-writes the exact post-#651 emission twin
  (empty struct + assoc helpers + dispatcher set calls in begin_play/tick)
  against a registered `LiveDispatchProbe` class (VmProbe-style manual
  EngineClass + WorldComponentRegistration submissions).
- Test 1 drives registration → begin_play through the real
  `ActorRegistry::register` path (42.0 lands in the shared store), arms a
  SceneDB#47 subscription, runs a real `TickLoop::tick_once` phase-2 tick
  (77.0 visible through the store) and asserts the `Mutated`
  ComponentChangeEvent fires for (actor entity, probe cid) — "visible in
  SceneDB subscriptions during standalone play".
- Test 2: hydration gate — absent component seeds baked default (10.0);
  present scene value (99.0) survives untouched.
- Test 3 ties twin to generator: PBGC output for the same class must contain
  the very calls the twin makes and none of the retired routing.
- Plus textual assertions where the emission is produced:
  `pbgc` integration test `rust_emission_routes_comp_ops_through_the_live_dispatcher`
  (dispatcher calls present; `__bp_with_comp`/`ComponentStore`/`gamma_core`
  absent) and updated drift-guard/probe emission tests.

**The e2e guard caught one real bug mid-flight** (this is why rule-5
scoped verification matters): my first template change made the Actor impl
forward `(_entity, _world)` while `tests/generated_project_compiles` fed
HAND-WRITTEN zero-arg logic into `CompiledBlueprint::new` → E0061 in the
generated project. Contract settled: `CompiledBlueprint` wraps ALREADY-
COMPILED PBGC logic, which since #651 carries the params — so the e2e test
and the drift guard now build a real graph and run `pbgc::compile_graph`
first (higher fidelity anyway: they exercise the true editor chain), and
pbgc's own test fixtures use param-bearing raw sources.

**Verification (scoped, rule 5)**: pbgc 18 lib + 28 integration + 2 doctests;
pulsar_world_registry 23 lib + 10 integration; pulsar_game lib 68/68 (3 new
probe tests) + e2e `generated_project_compiles --ignored` exit 0 (freshly
generated project with the new emission cargo-checks green against current
pins, ~11 s warm); engine_backend lib 78/78; clippy clean on every touched
file (remaining warnings pre-existing in untouched files). Cargo.lock user
WIP (twox-hash + scenedb-source lines) excluded via surgical staging; helio
pointer untouched; blueprint_editor submodule (user compiler.rs WIP) NOT
touched and needs no changes — pbgc's public API signatures it calls are
unchanged.

**Notes for #653 / F**

- `pulsar_game::time::to_scenedb_time` unchanged and still THE time seam;
  nothing here consumes GameTime.
- Generated code now names `pulsar_world_registry::{dispatch,marshal}`,
  `serde_json`, `tracing`, `pulsar_reflection` — all already in
  GAME_DEPENDENCIES; if you add more direct crate references to emission,
  extend that list in the SAME commit (E2's rule, re-proven here).
- For F's node UX: comp_call outputs arrive as JSON in the graph domain
  (`any_to_json` of the boxed return); get/set likewise. A graph-side typed-
  value story would need the D4 known-limit design (heap-typed variables),
  not new dispatch work.
- Template regeneration recipe (for future template refreshes): temp crate
  depending on pbgc by path, build GraphDescription, `compile_graph` →
  `generate_blueprint_actor_source[_with_components]`. Do NOT feed raw
  hand-written event fns into `CompiledBlueprint::new` expecting them to
  pair with the template — compiled-logic contract, see surprise above.

**Surprises / deviations**

- Issue said "update core_project_builder.rs:508-512" — those line numbers
  predate phase A; the flow was already correct post-A2, so the builder
  change reduced to documentation. Honest no-op, verified by the e2e check.
- The retired thread-local helpers themselves live in PINNED scenedb
  (`component_store.rs`) and cannot be deleted from here; deletion stopped
  at "no users + no re-export" (grep-verified).
- `has_custom_events` scaffolding kept as-is (comment-only `__init_events`);
  dead but harmless, and removing it belongs with F's real custom-event
  design (needs an actual bus crate — see E2's gamma_core note).

### Handoff: E2

Branch `scripting-epic`, commit `a5ad5f44` (parent repo) + pbgc submodule
commit `bc9929d` (inside `crates/third-party/pbgc`, on top of D's `fccd2d9`;
parent gitlink deliberately NOT bumped — same protocol as phase D, the user
pushes/bumps pointers). Issue #652.

**What landed**

1. **Emission aligned with the pinned trait.** pbgc's `gen_blueprint_actor`
   now emits `fn tick(&mut self, _entity: Entity, _world: &mut World)` —
   byte-identical to pinned SceneDB's deliberately time-free `Actor::tick`
   (verified equal at pin `201d35e0` AND the old `c761890`; the trait doc
   states timing is the engine's concern). The stale "fully-qualify GameTime"
   emission comment is replaced by a pointer to the drift guards.
2. **Phantom crate removed from emission.** Every generated actor struct
   carried `pub events: gamma_core::EventBus` — but NO crate named
   `gamma_core` exists anywhere in the graph (no lockfile entry, no EventBus
   type anywhere), so every generated project failed to compile on ANY class.
   Field deleted (zero consumers; custom events emit empty `__init_events`
   bodies today). Reintroduce only against a crate that actually ships an
   event bus.
3. **GameTime bridge made explicit.** The conversion moved out of tick.rs into
   `pulsar_game::time` (`pub fn to_scenedb_time`, field-verbatim, unit
   tested). This is THE seam — A's downstream contract #5 path is now real
   and public at `pulsar_game::time::to_scenedb_time`. True unification
   (single type) needs an upstream move (scenedb aliasing core or vice
   versa); orphan rule forbids local From impls, so explicit bridge it is.
4. **Drift guard, layer 1 (always-on)**:
   `pulsar_game::blueprint_codegen_drift` — (a) a hand-written REFERENCE
   actor mirroring PBGC's emitted shape compiled against the REAL pinned
   trait + driven through the real TickLoop (trait change ⇒ this stops
   compiling); (b) runtime assertions that generated output contains
   byte-identical signature constants and names no phantom crate (`GameTime`,
   `gamma_core`); (c) layout contract test for the class-tree files.
5. **Drift guard, layer 2 (end-to-end, `#[ignore]`)**:
   `cargo test -p pulsar_game --test generated_project_compiles -- --ignored`
   (= `just ci-drift-check`; fast probes = `just ci-drift-probe`). Generates a
   COMPLETE game project via `core_project_builder::ensure_core_bootstrap` +
   pbgc's public API into a temp dir and runs `cargo check` with a shared
   incremental target dir (`%TEMP%\pulsar_drift_check_target`). First run ≈
   70–90 s warm-deps, minutes cold; later runs seconds.

**The checks caught real rot on their first true run — fixed here:**
- engine_backend/build.rs baked Windows `\\?\` canonicalize prefixes into
  path deps → cargo `invalid path url` on EVERY freshly generated project
  manifest. Prefix now stripped (`absolute_path`).
- Generated-game manifests lacked `pulsar_world_registry`, which the
  EngineClass derive's generated code references since phase C ⇒ E0433 in
  every generated class. Added to GAME_DEPENDENCIES.

**Vendored-pbgc ↔ pinned-rev update procedure** (also in pbgc project.rs
module doc): upstream PBGC change → pull INTO the vendored copy → adjust
emission in the SAME change as any SceneDB rev bump → run BOTH guards
(`just ci-drift-probe && just ci-drift-check`) → user pushes submodule +
bumps parent gitlink. Codegen and pins move together; forgetting fails CI.

**Deviations / surprises for the record**
- The ideal always-compiled probe (build-script codegen + include!) was
  attempted and REJECTED: making pbgc a build-dependency of pulsar_game
  double-builds its pulsar_std chain as host+target units which collide on
  host==target (Windows) — E0460/E0463 chaos. Fallback design per issue text
  instead. Do NOT add pbgc as a build-dep of anything linking pulsar_std.
- Mid-session the user landed `330fa154` (SceneDB patch bump to `201d35e0` +
  Helio submodule) — my e2e check was re-run cold against the NEW pin and
  passes; the old local-path `[patch]` override caveat is gone, so generated
  manifests are CI-resolvable again.
- HEAD's Cargo.lock still lists `pulsar_scenedb` at source rev `c761890`
  while the committed manifest pins `201d35e0` (unstaged worktree hunks fix
  it) — pre-existing, left for the user, but `--locked` builds will fail
  until they stage those lines.
- Staged surgically: Cargo.lock carries ONLY the pulsar_game dependency block
  (+5 lines incl. predecessors' previously-unstaged-but-required regular
  deps), NOT the user's `twox-hash` hunk or scenedb-source-line hunks.

**Verification (scoped, rule 5):** pbgc 16 lib + 27 integration + 2 doctests,
pulsar_game lib 65/65, engine_backend lib 78/78, e2e generated-project cargo
check exit 0 (cold target dir); clippy clean on all touched files (remaining
pbgc/pulsar_game warnings are in untouched pre-existing code).

**Notes for #651 / #653**
- #651 (templates): Template-Blank still holds PRE-E2 generated shapes
  (three-param tick, gamma_core bus field, old engine_main register call).
  Regenerating via the editor fixes them; if templates contain HAND-written
  actors, update their tick signatures manually. Generated manifests now need
  nothing manual (path-url + world_registry fixes are in the builder).
- #653: `pulsar_game::time::to_scenedb_time` is the public seam; script-crate
  authoring UX should funnel any time conversion through it. The reference
  actor in `blueprint_codegen_drift` doubles as a live example of minimal
  generated-actor shape (struct + Default + Actor impl) for docs/samples.
- If a future EngineClass derive gains more direct crate references, extend
  GAME_DEPENDENCIES in engine_backend/build.rs IN THE SAME COMMIT — the e2e
  check will catch it otherwise (that is how this one was found).
