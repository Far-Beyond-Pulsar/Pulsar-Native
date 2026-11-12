# Component lifecycle refactor: audit and plan

Status: **Phase 0 (this document): design record and audit. No code changes.**
Every claim about current code below was read in this change, with `file:line`
(lines are at the commit this branch is based on, `d100b68a3`). Anything not
read is marked **UNVERIFIED** (section 7). The decisions in section 4 are
**Proposed, needs the user's confirmation**; nothing in sections 2 and 5 is
final until they are answered.

Constraints already decided with the user (not reopened here):

- SceneDB must know nothing about traits, scripts or Blueprints.
- Lifecycle traits must **not** be reachable from Blueprints. That is guaranteed
  by construction (a separate capability registry, not reflected methods) plus an
  audit test, not by a flag.
- The file-based trait DB (`pulsar_traits`: roles, `NativeTraitImpl`) stays
  separate and discoverable.

The goal: one component-level lifecycle ("Actor"-style events `begin_play`,
`end_play`, `tick`, and a new `construct`) that replaces SceneDB's own
`Actor`/`ActorRegistry` and the raw-`inventory` `ComponentTickRegistration`, so
physics, Blueprints and any plugin share one foundation.

## 0. Findings that change the plan (read these first)

1. **Helio's `Actor` is a different trait.** `crates/renderer/helio/crates/helio/src/lib.rs:22`
   re-exports `helio_core::Actor`, which is a two-method render-side trait
   (`entity()` / `set_entity()`, `helio-core/src/actor.rs:3`), **not** SceneDB's.
   So the Helio re-export is not a consumer of `pulsar_scenedb::Actor` and P5 does
   not repoint it. It does mean there are **three** things named `Actor` in the
   graph (SceneDB's, Helio's, the new one). Naming consequence in Decision D1.
   Helio is still touched by P5, but only for its own SceneDB rev pin
   (`crates/renderer/helio/Cargo.toml:64-65`).
2. **SceneDB rev is pinned in three manifests, not two**: root `Cargo.toml:406-407`
   and `:807-808` (the `[patch]`), and Helio's `Cargo.toml:64-65`. No plugin
   manifest names the rev (grep of every `Cargo.toml` for the rev found only
   those three files). Generated game projects get the rev baked from the root
   manifest (`crates/core/engine_backend/build.rs:22`) and stale entries are
   replaced on regeneration (`core_project_builder.rs:355-360`).
3. **`ActorRegistry` never ends actors at session end.** Nothing in this repo
   calls `ActorRegistry::deregister` outside tests (`pulsar_game/src/tests.rs:135`);
   `TickLoop` has no actor shutdown (`tick.rs:683-699`, `:828-832`) and
   `ActorRegistry` has no `Drop` (`actor.rs` in SceneDB, whole file read). So a
   generated Rust Blueprint actor's `end_play` (which calls `script.end_play`,
   `pulsar_script_codegen/src/actor.rs:276-278`) does **not** run when a PIE or
   standalone session stops, although SceneDB's trait doc says it runs "on
   deregistration or shutdown". P4/P5 fix this as a side effect; it is a
   behavior change to call out in release notes.
4. **PIE has no separate world.** The embedded game's `TickLoop` adopts the
   editor's own `SceneDb` (`embed.rs:384`, `TickLoop::with_scene_store`); Play
   snapshots it (`scene.rs:459`) and Stop restores the snapshot after the game
   shut down (`scene.rs:487`, `pie.rs:178-224`). So an "Editor" construct pump
   and a "PlayInEditor" session act on the **same** world at the same time; one
   of them must own it (P2 design rule, section 2.4).
5. **Native component `begin_play` is detected by scanning, not by journal.** The
   derive's shim iterates a typed query each tick and calls `begin_play` for rows
   absent from the previous set (`engine_class_derive/src/component_runtime.rs:345-360`).
   Only removals use journal cursors (`component_lifecycle.rs:219-229`, `:311-361`).
   So a level-file component already begins on the first tick after the level is
   present. "Begin through the journal path" therefore means: the session opens
   its cursors, does one initial scan that enqueues every live instance as
   `Added`, and thereafter reads only the journal.
6. **`#[register_component_runtime]` is registered by raw `inventory::submit!`**
   (`component_runtime.rs:371-380`), not through `ClassSpec`; its gate is by class
   name (`pulsar_world_registry/src/runtime.rs:151-153`). The registry's ABI
   fingerprint hashes `size_of::<ComponentTickRegistration>()` (`runtime.rs:37`),
   so any new registration type or a changed layout bumps `ABI_VERSION`
   (`runtime.rs:28`, currently 3).
7. **Only two shipped plugins use the component-runtime macro**: voxel
   (`plugins/vendor/voxel/component/src/component_runtime.rs:16`) and skm_editor
   (`plugins/vendor/skm_editor/component/src/component.rs:1574`), plus one engine
   test (`pulsar_game/tests/component_event_routing.rs:34`). **Physics does not**:
   it is a `System` plus journals (`physics_plugin/src/registrations.rs:50`).
   P6's migration cost is small.
8. **Physics is inert in the editor already** and its world settings are dead.
   Systems run only inside `tick_live_components` (`component_lifecycle.rs:251-264`),
   called only by `TickLoop::run_tick` (`tick.rs:456`) and tests. The
   `physics_enabled` / `gravity` World Settings fields are registered only under
   `#[cfg(feature = "editor")]` (`registrations.rs:113-132`) and nothing reads
   them; gravity is the constant `GRAVITY` (`sim.rs:35`, used at `sim.rs:194`).
9. **Removal carries no component data.** SceneDB's journal entry is
   `{ entity, kind }` only (`change_journal.rs:86-89` in SceneDB), and
   `end_play` is documented as owner-only after removal
   (`pulsar_world_registry/src/lib.rs:100-102`, `component_lifecycle.rs:363-368`).
10. **Hand-written native script crates are the only user code that `impl Actor`.**
    Generated Blueprint classes are regenerated ("Generated. Do not hand-edit",
    `pulsar_script_codegen/src/actor.rs:228,321`) and the project's `engine_main` is
    always regenerated (`core_project_builder.rs:1150`). The hand-written
    surface is the scaffolded `Spinner` (`core_project_builder.rs:908-921`) and
    whatever users wrote from it. This narrows the deprecation window needed in
    Decision D3.

## 1. As-is audit

### 1.1 SceneDB's `Actor` and `ActorRegistry`

File: `~/.cargo/git/checkouts/scenedb-*/017252c/crates/pulsar_scenedb/src/actor.rs`
(123 lines, read whole). The three checkouts present locally all contain rev
`017252c`, which is the pinned one (root `Cargo.toml:406`).

| Item | What it is |
|---|---|
| `trait Actor: Send + Sync + 'static` | `begin_play(&mut self, Entity, &mut World)`, `end_play(&mut self, Entity, &mut World)`, `tick(&mut self, Entity, &mut World)`, all default no-ops. `tick` is deliberately time-free (no delta). |
| `ActorRegistry` | `Vec<ActorEntry { entity, actor: Box<dyn Actor>, alive }>`. `register` spawns the backing entity, calls `begin_play` **immediately** (so before the window/world is ready unless the caller defers), then stores it. `deregister` calls `end_play`, despawns, drops. `tick_all` ticks alive entries under a profile scope. |
| What it knows | `Entity`, `World`. It is not tied to a component: an actor owns one spawned entity and is a boxed object outside the world. |

It is exported from SceneDB's root (re-exported by `pulsar_game`, below).

### 1.2 Every consumer in this repo

| Consumer | `file:line` | How it uses SceneDB's `Actor` |
|---|---|---|
| `TickLoop` | `pulsar_game/src/tick.rs:6,14,48,219,269` | field `pub actors: ActorRegistry`, built in both constructors (`:219`, `:269`). |
| tick phase 2 | `tick.rs:465-483` | `actors.tick_all(&mut store.world)`, then ticks hot-reload "rebound shells" in the same write lock; then `flush_component_events`. |
| script-crate wrapper | `pulsar_game/src/scripts.rs:25,50-70,86-110` | `TickLoop::register_actor<A: Actor>` stamps a `ScriptTag`, and on hot reload binds a `ReboundActor { entity, actor: Box<dyn Actor> }` to an existing entity because the pinned registry "can only box actors it spawned itself" (`scripts.rs:50-52`). |
| re-exports | `pulsar_game/src/lib.rs:20,173` | `pub use pulsar_scenedb::{Actor, ActorRegistry, ...}` and again in the prelude. This is why generated code and user crates write `use pulsar_game::{Actor, Entity, World}`. |
| codegen | `pulsar_script_codegen/src/actor.rs:271-290` | emits `impl Actor for {ty}` with `begin_play`/`end_play`/`tick` forwarding to the exported script; comment `:279-285` says the signature "MUST match the pinned `pulsar_scenedb::Actor` exactly". |
| project builder | `engine_backend/src/services/core_project_builder.rs:908,921` (scaffolded `Spinner`), `:1134,:1167` (doc on generated `engine_main::setup` registering into `game.actors`) | a hand-written actor template and generated setup text. |
| drift guards | `pulsar_game/build.rs:1-10`; `src/blueprint_codegen_drift.rs:4-5,25-30,55`; `src/export_probe.rs:3,18-35,51-53` | generate one actor through the real generator and compile it against the pinned trait; byte-compare the emitted `tick`/`begin_play` signatures to a reference twin. Drift is a compile error in `cargo test -p pulsar_game`. |
| generated-project test | `pulsar_game/tests/generated_project_compiles.rs:252` | a probe binary does `use pulsar_game::prelude::{Actor, Entity, World}` and calls `begin_play`/`tick` with `Entity::DANGLING`. |
| engine tests | `pulsar_game/src/tests.rs:104-135,155` | `impl Actor for Counter`; `actors.register`/`deregister` (the only `deregister` caller). |
| Helio re-export | `crates/renderer/helio/crates/helio/src/lib.rs:22` | **not a consumer** (finding 1): it re-exports Helio's own render-side `Actor`. |
| unrelated name hits | `engine_backend/src/subsystems/world/mod.rs:31` (a struct `Actor`, spatial vault payload); `pulsar-relay` CRDT docs; script object model `ActorRef` (`SCRIPTING_EPIC.md`) | different things named actor; no change. |

Docs that describe SceneDB's `Actor` and must be updated when it goes:
`.agents/ECS.md:74-84`, `.agents/CRATES.md` and `.claude/commands/blueprint-system.md` (one word-match each, context not read).

Rev pins: root `Cargo.toml:406` (`pulsar_scenedb`, with `features = ["gpu"]`),
`:407` (`pulsar_scenedb_derive`), `:807`, `:808` (the `[patch."…/SceneDB"]`
block whose `//` spelling lets a patch name another rev of the same repo, comment
at `:800-806`), and Helio `Cargo.toml:64-65`.

### 1.3 The native component lifecycle

| Piece | Where | Facts |
|---|---|---|
| `ComponentTickRegistration` | `pulsar_world_registry/src/lib.rs:79-105` | `inventory::collect!`ed struct: `type_name`, `class_name`, `handler_events`, `component_type: fn() -> ComponentId`, `tick: fn(&mut World, &EventHub, f32, &HashMap<Entity,Entity> /*previous*/, &mut HashMap<Entity,Entity> /*current*/, &mut ComponentRuntimeState)`, `end_play: Option<fn(Entity, &EventHub)>`. The `tick` fn is a whole-class loop, not a per-instance callback. |
| Registry plumbing | `runtime.rs:72-89,122-126,151-153,255-257` | `TICKS` append list; gated by `class_enabled(class_name)`; handed across DLLs through the `Runtime` fn table (`ABI_VERSION` 3, `:28`, fingerprint `:30-53`). |
| `ComponentRuntimeState` | `component_lifecycle.rs:31-54` | "Lifecycle bookkeeping for one running game session", identities only, owned by `TickLoop` (`tick.rs:64,223,273`). Fields: scene identity, `active: class -> {instance entity -> owner entity}`, event inbox, Gamma subscriptions, per-type removal cursors, scratch, cached system order. Its own doc says it must not be global so independent PIE/game worlds do not share activation state (`:33-36`). |
| `tick_live_components` | `component_lifecycle.rs:204-266` | If the scene allocation changed, `end_all`. Then `process_component_removals`; open one change cursor per registered type (first time); compute and cache the system order; walk steps: `Step::Components` -> `tick_component_lifecycles`, `Step::System(r)` -> `(r.system.run)(&mut SystemContext {world, events, delta_seconds, runtime})`. |
| per-class loop | `:270-302` | calls the class shim with `previous`, collects `current`, `ended = previous - current` (sorted by entity bits), calls `end_play(owner, events)` for each, unsubscribes. |
| removals | `:311-361` | reads each type's cursor; for each `Removed` entry whose instance is active, calls `end_play(owner, events)`. An overflowed journal ends nothing here; the next tick's set-difference ends what is gone (`:306-309`). Called after the schedule/actor phase (`tick.rs:490`) and after scripts (`tick.rs:535`). |
| end of session | `component_lifecycle.rs:149-180,369-371`; `tick.rs:697-699`; `tick.rs:623` (after `run_blocking` ends) and `tick.rs:830` (`Drop`) | `end_all` calls `end_play(owner, events)` for every active instance, then clears all state. Not called from `PieSession::shutdown` (`embed.rs:199-202`, which only calls `end_scripts`); it runs when the `TickLoop` is dropped (see 1.6). |
| the derive | `engine_class_derive/src/component_runtime.rs:27-53,238-287,291-382` | Validates exact signatures: `fn tick(&mut self, &mut ComponentContext, f32)` (required), `fn begin_play(&mut self, &mut ComponentContext)` (optional), `fn end_play(entity: Entity, &mut ComponentContext)` (optional, an associated fn with no `self`, `:276,282`), `#[bp_handler("event")]` methods. Emits an `end` shim (`:313-316`), the tick shim (query `(&mut T, &ComponentOwner)` at `:345-349`, skips disabled instances `:349`, `enabled = field` option `:322-326`) and the `inventory::submit!` (`:371-380`). |
| `ComponentContext` | `lib.rs:241-258` | `{ entity: owner, events: ComponentEventWriter }`. "Intentionally no `World` reference: handlers can therefore never re-enter a World borrow." |

**What the callbacks receive, exactly.**

- `begin_play(&mut self, &mut ComponentContext)` and `tick(&mut self, &mut
  ComponentContext, f32)`: the live component row mutably (borrowed in place from
  SceneDB through the typed query), the owner entity, the instance entity, and a
  deferred event writer. No `World`, no session handle, no other components.
- `end_play(entity: Entity, &mut ComponentContext)`: **no component**. It gets the
  owner entity (`end_shim`, `:313-316`) and a context built with
  `ComponentContext::new(entity, events)` (no instance). It cannot read the
  removed component because SceneDB already dropped it, and the journal has no
  payload (finding 9). It also cannot read any other component through the
  context. What it **can** do: drop owner-keyed runtime state, unsubscribe,
  publish events.
- Disabled instances are treated as ended (`ComponentOwner::is_enabled`
  `attachments.rs:119`; the shim `continue`s, so they leave `current` and
  `end_play` fires).

**Users of the macro / of the state types** (grep over `crates/` and, from the
main checkout, `plugins/`; submodules are not initialized in this worktree, see
section 7):

| User | Where |
|---|---|
| voxel | `plugins/vendor/voxel/component/src/component_runtime.rs:16` (`class = "VoxelTerrainComponent", enabled = enabled`) |
| skm_editor | `plugins/vendor/skm_editor/component/src/component.rs:1574` plus many test files driving `tick_live_components` and `ComponentRuntimeState::default()` (`component.rs:1697`, `phase3_tests.rs:230`, `notifies.rs:1729`, ...) |
| engine tests | `pulsar_game/tests/component_event_routing.rs:34,101-106`; `plugins/installed/tests/character.rs:155-159`; `component_lifecycle.rs:373-432` |
| physics tests | `physics_plugin/src/runtime.rs:471-780` call `tick_live_components`/`ComponentRuntimeState` to drive the physics `System` |

### 1.4 How a class declares things today

- **`ClassSpec`** (`engine_backend/src/scene/class_spec.rs:38-49`): `requires`
  (classes attached together), `systems: &[System]`, `gpu` (feature `render`),
  `presentation`. Registered from a plugin root as
  `p.components().class::<T>(ClassSpec { .. })` (`:75-96`). It adds the class's
  `WorldComponentRegistration` and one `SystemRegistration` per system, with the
  plugin as origin. It has **no** lifecycle field.
- **`System`** (`pulsar_world_registry/src/systems.rs:46-76`): `label`, `after`,
  `before`, `run: fn(&mut SystemContext)`. The component lifecycles are the node
  label `pulsar.components` (`:34`) of the same order (`order`, `:122-189`).
  Systems run only while their class is registered (gate by class,
  `runtime.rs:163-166`). `SystemContext` carries `world`, `events`,
  `delta_seconds` and the session's `ComponentRuntimeState` (`:37-43`).
- **Where lifecycle comes from today**: the raw `inventory` registration of
  `#[register_component_runtime]` (1.3), **separate** from `ClassSpec`.

### 1.5 Script/class instances, `begin_play`, PIE

Frame order in `TickLoop::run_tick` (`tick.rs:411-589`): flush (after input);
phase 0 plugin `before_systems` (animation, `:430-437`); phase 1 `Schedule`
(`:443-445`); phase 1b `tick_live_components` incl. systems such as physics
(`:453-463`); phase 2 `actors.tick_all` + rebound shells + `flush_component_events`
(`:472-483`); `process_component_removals` (`:488-495`); flush (after systems);
phase 3 script driver `run_frame_shared` (`:512-528`); `process_component_removals`
again (`:533-540`); plugin `after_scripts` (`:545-561`); flushes; `pump_assets`;
`end_change_window` (`:585`).

The script driver (`pulsar_game/src/scripting/driver/`):

- Follows the world's `ClassInstance` components with its **own** two change
  cursors (`ClassInstance`, `ComponentOwner`; `reconcile.rs:125-173`, opened at
  `:161-162`), falling back to a full `rescan` on overflow (`:188-216`).
- `sync_entity` (`:218-257`) starts, restarts (class changed) or stops the object's
  script; `start`/`try_spawn` (`:259-363`) call `runtime.spawn_with_json`, bind
  class slots and subscribe; `stop` (`:365-400`) despawns the script instance and
  publishes `EndPlay` if it had begun.
- `begin_play` is **queued** by `ScriptRuntime::spawn` (`pulsar_script_runtime/src/lib.rs:890-938`,
  `pending_begin_play`) and run later by `dispatch_pending_begin_play`
  (`:1083`) from `frame_start` (`driver/frame.rs:97-112`), which then publishes
  `BeginPlay`. `has_begun` (`:1240`) decides whether `stop` publishes `EndPlay`.
  The deferral is deliberate: begin_play must see a fully initialized world
  (`tick.rs:504-511`).
- `end_play_all` (`reconcile.rs:61-104`): ends possession, calls the game mode's
  `on_game_end`, publishes `GameEnded`, runs every enabled `GameSessionRegistration::end(world)`
  hook (`:89-93`), `runtime.end_play_all`, then despawns the session's role objects.
- Globals and role objects are also started by the driver (`start_globals`
  `:402-443`, `start_roles`), not tied to a component.

Session start and end:

- **Standalone / headless**: `run_blocking` (`tick.rs:601-624`) ends with
  `end_scripts()` then `end_component_runtime()`; headless run in `standalone.rs`
  ends scripts at `:384`; `TickLoop::drop` also ends components (`tick.rs:828-832`).
- **PIE**: the editor's `begin_pie` snapshots the world and enters play mode
  (`ui/panel/pie.rs:27-44` -> `state/scene.rs:459-469`: snapshot, remember live
  entities, `EditorMode::Play`), saves a temp level, builds the game dylib. The
  guest `EmbeddedGame::new` adopts the editor's `SceneDb` (`embed.rs:384`),
  arms hot-reload rebinding if the host flagged `RELOAD` (`:392-396`), runs
  project `setup()` (`:408`), loads world roles (`:414`). Stop: `end_pie`
  (`pie.rs:178-197`) sets `stop_requested`/`restore_after_stop`; the viewport
  stops the host (`PieHost::stop`, `engine_backend/src/services/pie_host.rs:612-632`)
  which calls the guest `pulsar_pie_shutdown` (`embed.rs:806-815`):
  `session.shutdown()` (= `end_scripts`, `embed.rs:199-202`) then `drop(game)`,
  which drops the `TickLoop` and so runs `end_component_runtime` (`tick.rs:830`).
  Only afterwards does `finish_stop` call `exit_play_mode` (`pie.rs:205-224`,
  `scene.rs:487-515`): despawn entities not in the pre-Play set, then restore the
  snapshot. **Current order is therefore correct but accidental**: component
  `end_play` runs inside the `TickLoop` drop, after scripts, and with no `World`.
- **Native dylib hot reload** (#653): pressing Play while running stops the old
  host first (so every script and component gets `end_play`) and starts the new one
  against the same world with the `RELOAD` flag (`game_viewport.rs:407-414`, the
  comment block "Native hot reload (#653)"). New session = `begin_play` again.
  **`scripts_only`** reload (`game_viewport.rs:389-401`) keeps the host and only
  reloads classes (`reconcile.rs:112-116`); no lifecycle event may re-fire there.

### 1.6 How Blueprint discoverability works

`pulsar_world_registry/src/script_natives.rs` registers, for **every** class in
`runtime::world_components()` (`:49-69`, `:62-78`):

- `Class::get_<property>` / `set_<property>` for every reflected property
  whose type has a script binding (`:96-155`);
- `Class::<method>` for every reflected method whose parameter and return types
  have script bindings (`:158-243`).

The only filter is "script-representable types" (`:100-106`, `:173-196` debug-log
and skip). `MethodFlags` is `{ side_effect_free, deterministic }` only
(`pulsar-reflection/.../methods.rs:100-105`): **there is no hidden/internal bit**.
The class's component-event handlers are separate (`ComponentEventRegistration`).
So any reflected method named `tick` or `begin_play` on a world component would
be a Blueprint-callable native today.

Audit precedent: `find_overloaded_methods` (`audit.rs:44-67`) sweeps
`REGISTRY.get_class_names()` and `REGISTRY.get_methods(class)` for duplicate
method names, with a unit test asserting the live registry is clean
(`overload_sweep_reports_only_real_collisions`, `audit.rs` tests module), and
`metadata_snapshot_json` (`:77`) for golden snapshots. It reads only public
reflection registries, never mutates them (`audit.rs:17-19`). The lifecycle leak
audit follows this shape (section 2.6).

The trait DB is a different, intentionally Blueprint-visible mechanism:
`pulsar_traits` (`NativeTraitImpl`, `sources.rs:131-159`; `TraitImplementerSource`,
`lib.rs:12`) and the driver's `call_trait` (`driver/roles.rs:441`).

### 1.7 The physics plugin's runtime

Read from the main checkout (`plugins/vendor/physics_plugin`, pinned
`714440839e`). Files: `src/runtime.rs` (787 lines), `registrations.rs`
(194), `sim.rs` (222).

| Fact | Where |
|---|---|
| The driver is **one** `System`, `pulsar.physics.step`, after `pulsar.components`, attached to the rigid body class's `ClassSpec` | `registrations.rs:50,61-65` |
| Per-instance callbacks are explicitly not used: they "cannot see the other rows that colliders, joints and force fields relate to" | `runtime.rs:12-14` |
| State is a global `static SESSIONS: LazyLock<Mutex<HashMap<usize, Session>>>` keyed by the **address** of the `World` | `runtime.rs:238-242` (`key_of`) |
| Comment admits a reused address "starts from a stale session" and relies on a full rescan | `runtime.rs:234-237` |
| Idle sessions are pruned only if nobody ticked for 10 s (`IDLE_SESSION`), checked on a *later* tick of any world | `runtime.rs:57,258` |
| `Session::drop` sets `shutdown` and joins the `Pulsar-Physics` service thread (so a session ends when its map entry is dropped) | `runtime.rs:158-165`, thread loop `:168-222` |
| There is **no session end hook**: the plugin registers no `GameSessionRegistration` (only `input_plugin` does, `input_plugin/src/registrations.rs:33`) | grep of `plugins/` |
| `GameSessionRegistration::begin` is `fn(hub: &EventHub, project_root: &Path)`, with **no world**; `end` is `fn(world: &World)`; `possess` gets a `PossessionContext { project_root, roles }` | `engine_backend/src/scene/game_hooks.rs:144-167` and the context type just above it |
| Body/collider/joint existence is derived from change journals: per-behavior cursors opened on first use, `Removed` entries -> `touched.removed`, `Overflowed` -> rescan | `runtime.rs:303-336` (`read_moves`), `:378-425` (`read_parts`) |
| Results come back a frame late through an outbox and are fenced against moves by others | `runtime.rs:338-376` (`apply`) |
| `Sim::run(parts, dt)` steps Rapier with the constant `GRAVITY` | `sim.rs:35,165,194` |
| `physics_enabled` and `gravity` are registered as World Settings fields only with the `editor` feature and are read by nothing | `registrations.rs:113-132`; grep over `crates/core`, `crates/editor`, `plugins` found no reader |
| Public diagnostics keyed by world address: `simulated_counts(&World)`, `wait_idle(&World)` | `runtime.rs:429,441` |
| Level settings reach a plugin through `WorldRoles::setting(key)` (the input plugin's mapping) | `pulsar_traits/src/roles.rs:270`; hosted on the loop as `world_roles` (`tick.rs:72`), loaded in PIE at `embed.rs:414` |

### 1.8 Layering today

```
SceneDB (external repo)    World, Entity, journals, Actor/ActorRegistry   <- Actor lives here today
pulsar_scene_model         ComponentOwner, attachments
pulsar_world_registry      WorldComponentRegistration, ComponentTickRegistration (raw inventory),
                           System, ComponentRuntimeState, tick_live_components,
                           script_natives (reflection -> Blueprint natives), audit
engine_backend             ClassSpec, game_hooks (TickPhase/GameSession/...), SharedScene
pulsar_game                TickLoop, script driver, embed/standalone
pulsar_traits              file trait DB, roles, NativeTraitImpl
```

`ComponentTickRegistration` and `script_natives` sit in the **same crate**, so
today nothing but convention keeps lifecycle fns out of the Blueprint surface.

## 2. Target design

### 2.1 (a) Capability registry outside SceneDB

A capability registry is a per-`ComponentId` table of function pointers **per
trait**, registered next to `ClassSpec` from the plugin root and gated by the same
plugin gate (`pulsar_plugins::Contribution` with an `origin`, like
`SystemRegistration`, `systems.rs:87-108`).

- A trait is described by a small marker with a table type, e.g. `Actor` ->
  `ActorTable { construct, begin_play, tick, end_play: Option<fn(..)> }`. The
  registry stores `(ComponentId, class_name, origin, ActorTable)` and offers
  lookup by `ComponentId` (sorted slice, binary search) and iteration in a
  deterministic order (class `type_name`, as today `component_lifecycle.rs:221-222`).
- The new lifecycle trait is named **`Actor`** by default (user's wish). While
  SceneDB's `Actor` still exists it lives only under a module path
  (`pulsar_component_lifecycle::Actor`, or `pulsar_world_registry::lifecycle::Actor`
  per D6); it is **not** added to `pulsar_game`'s prelude until P5 removes SceneDB's,
  so `use pulsar_game::prelude::*` never sees two. See D1 for the Helio clash.
- Registration: `ClassSpec` gets one more field,
  `actor: Option<ActorTable>` (`ActorTable::of::<T>()` built from the component's
  `impl Actor for T`), and `Components::class` adds one `ActorRegistration`
  contribution (`class_spec.rs:75-96`). `Option` keeps `ClassSpec::default()` and
  every existing `..ClassSpec::default()` call valid.
- A table is **per class, not per instance**: it holds `fn` pointers
  (`fn(&mut World, &mut ActorCx, Entity)` shaped by the framework, see 2.5), so it
  crosses the plugin DLL boundary like the other registries (through the
  `WorldRuntimes`/`Runtime` fn table, `ABI_VERSION` 3 -> 4, new type added to the
  fingerprint at `runtime.rs:30-53`).
- The existing `ComponentTickRegistration` stays during P1-P5 as a **legacy
  entry** the same orchestrator drives (the "adapter"): classes still registered
  through the raw inventory run through their old class-loop shim in the same
  `Step::Components`; classes with an `ActorTable` run through the new per-instance
  path. A class in **both** is a hard error reported at registry build (test).
  P6 changes the derive to emit an `ActorTable` registration instead and deletes
  the raw path.

### 2.2 (b) Events

| Event | When | Where it runs | Notes |
|---|---|---|---|
| `construct` | a component of the class is added or changed (property write, enable/disable, owner change) | **editor and play** | coalesced to at most once per frame per component instance (D4). Fires before `begin_play` of the same frame. Must be idempotent and cheap. New: nothing like it exists (grep for `construct`/`OnConstruction` found none). |
| `begin_play` | an instance becomes live **in a play session** (kind PlayInEditor/Standalone/Headless): present at session start, added/enabled later, class reloaded as a different class | play sessions only | Same path for level-file and runtime-spawned instances: the session opens its cursors, runs one initial scan that enqueues every live instance as `Added` **after the whole level is present** (finding 5), then reads only the journal. The script phase's "begin_play sees a fully initialised world" guarantee (`tick.rs:504-511`) is kept by session-ready gating: no `begin_play` until the host marks the session ready. |
| `tick` | every frame for live, enabled instances | play sessions only, in `pulsar.components` | `delta_seconds` as today. Never runs in the Editor session. |
| `end_play` | instance disabled, component removed, owner despawned, or **session end** | play sessions | entity-only plus owner (D2). Guaranteed for every live instance at session end (2.4). |

Ordering within a frame: `construct` (editor pump or play pump) -> `begin_play`
(new instances, in a deterministic order, D5) -> `tick` -> `end_play` for those
ended this frame, relative to the `System`s by label exactly as today.

### 2.3 (c) Session scope

- `Session` is the evolution of `ComponentRuntimeState` (already "one running game
  session", already passed to every `System` via `SystemContext.runtime`,
  `systems.rs:42`), owned by whoever drives the world: `TickLoop` for play,
  the editor's pump for Editor.
- `SessionKind::{Editor, PlayInEditor, Standalone, Headless}`, queryable from every
  callback context (`cx.session().kind()`, `kind.is_playing()`). Replaces ad hoc
  "am I in play" checks (none exist today; see Unity `Application.isPlaying`
  below).
- **End guarantee**: ending a session runs `end_play` for **every** live instance
  while the world is still alive, **then** ends scripts/session hooks, then drops
  session resources, **then** returns control to the host. In PIE the host
  restores its snapshot only after that returns (today's order, but made explicit
  in `PieSession::shutdown` instead of relying on `TickLoop`'s `Drop`;
  `embed.rs:199-202,806-815`; `tick.rs:830`). A test fails if any instance saw
  `begin_play` without `end_play` before the restore.
- **Session resources**: a type-keyed bag on the session
  (`get_or_insert_with::<T>()`), dropped with the session. This replaces physics'
  global address-keyed map (`runtime.rs:238-242`) and the idle-prune workaround
  (`:57,258`). Resource `Drop` is the shutdown hook (`Session::drop` joins the
  thread today, `runtime.rs:158-165`).
- **World settings in the session**: the host fills a read-only settings view
  (the level's `WorldRoles::setting`, `roles.rs:270`) at session creation, so a
  plugin reads `gravity`/`physics_enabled` without a new hook.

### 2.4 Ownership of the world between sessions (new rule)

Because PIE shares the editor's world (finding 4): at most one session **owns**
a world at a time. The Editor session's construct pump is suspended while a play
session is attached and the play session's pump delivers `construct` (so a
slider drag during Play still constructs, once per frame). On Stop the play
session ends (2.3), the snapshot is restored, and the Editor pump resumes; the
restore's mass component add/remove must not storm `construct` (the editor pump
treats a restore as one "rescan", D4).

### 2.5 Callback shapes and re-entrancy

Today's callbacks borrow the component mutably while the framework holds the
typed query, so they cannot be given `&mut World` (`lib.rs:237-240`). The design
keeps that safety and adds a controlled escape:

- Per-instance form: `fn begin_play(&mut self, cx: &mut ActorCx)` where `ActorCx`
  = `{ entity (owner), instance, session (kind, resources, settings), events
  (deferred), commands (deferred spawn/despawn/insert/remove, applied after the
  callback) }`. No `&World`.
- World form (opt-in, for components like physics that must see siblings): a
  second table entry `fn begin_play_world(world: &mut World, cx, instance)` that the
  framework calls **without** holding a component borrow (the component is
  fetched by the callback itself). A class picks one form per event.
- The Blueprint class-instance component uses the world form (the VM needs the
  world).

### 2.6 (f) Layering and the leak audit

```
L0  SceneDB                          data, journals. Knows nothing above it.
L1  capability registry (Actor ...)  tables by ComponentId. Knows L0 + EventHub; NOT reflection/script VM.
L2  reflection + script surface      pulsar_reflection REGISTRY, script_natives, Blueprint natives. Knows L0. Not L1.
L3  file trait DB                    pulsar_traits (roles, NativeTraitImpl). Separate, discoverable by Blueprint on purpose.
```

By construction: L1 is its own crate (D6) whose dependencies exclude
`pulsar_reflection` and `pulsar_script_vm`, and L2 (`script_natives`) has no edge
to it, so no code path from a Blueprint native can name a table. `just arch`
(`architecture/layers.toml`) gets a rule that L2 crates must not depend on L1 and
L1 must not depend on L2 (UNVERIFIED how `layers.toml` expresses a crate-to-crate
rule; see section 7).

Audit test (follows `find_overloaded_methods`, `audit.rs:44`): for every
registered capability table, take its **reserved method names**
(`construct`, `begin_play`, `tick`, `end_play`, plus any name the table registers),
and fail if any appears (a) in `REGISTRY.get_methods(class)` for any class, or (b)
as a native named `Class::<name>` in `world_component_natives()` output
(`script_natives.rs:62-78`). Because `MethodFlags` has no hidden bit, the test is
the only guard against a future `#[component_methods] fn tick` leaking a
lifecycle name into Blueprint; it must run in a crate that links real classes
(the plugin-aware test crate, `plugins/installed/tests`, per `AGENTS.md` "A test
that needs plugins belongs to `plugins/installed/tests`") so it covers physics and
the rest. A second test asserts the L1/L2 crate edge is absent (`cargo metadata`).

### 2.7 (d) Physics under the new lifecycle

- The world-wide step stays a `System` (`pulsar.physics.step`, after
  `pulsar.components`); it needs the whole world (`runtime.rs:12-14`).
- Body/collider/joint/force-field/character **registration** moves to
  `begin_play`/`end_play` of each component class (world form, 2.5), writing to
  the session's physics resource instead of being rediscovered from journals each
  frame. **Property changes during play still flow through journals** (edits of a
  running body); lifecycle owns existence, journals own mutation. This removes the
  `read_parts` special-casing of `removed`/`rescan` for existence (`runtime.rs:378-425`)
  but keeps `read_moves`/`apply` (`:303-376`) as is.
- Replace `SESSIONS` (`:238`), `key_of` (`:240`), `IDLE_SESSION` (`:57`) and the
  prune (`:258`) with a session resource that owns the `Session` and drops with it.
  The sim thread shuts down when its resource drops, and also **eagerly when the
  last physics component ends** (resource refcount), restarting lazily at the next
  `begin_play`. A panic restart (`runtime.rs:205-213`) is unchanged.
- `simulated_counts`/`wait_idle` (`:429,441`) take the session instead of `&World`.
- Gravity and `physics_enabled` are read from the session's settings at
  `begin_play` of the first physics component and passed to `Sim` (replacing
  `GRAVITY`, `sim.rs:35,194`); disabled means components never register and the
  step is a no-op. Mid-play settings changes are out of scope (UNVERIFIED
  whether settings can change live).

### 2.8 (e) Blueprint class instances

`ClassInstance` (`pulsar_class`, used at `reconcile.rs:8`) implements `Actor`
natively with the world form: `begin_play` = today's `start`/`try_spawn` (spawn
the script instance, bind slots, subscribe; the VM's own `begin_play` stays
queued and run in the script phase, `frame.rs:106`), `end_play` = today's `stop`,
`construct` = today's "same class, rewritten: rebind slots" (`reconcile.rs:250-252`).
The driver deletes its own cursors, `rescan`, `tracked`/`holders` maps and
`sync_entity` (`reconcile.rs:125-257`, `:365-400`). The driver keeps what is not
component-backed: globals, role objects, possession, class hot reload
(`reconcile.rs:112-116`), and runs as a **session resource** so the callbacks
reach it. This is what makes "the script driver stops following class instances
separately" true and also gives Blueprints `end_play` at session end for free.

## 3. How others do it (general knowledge, not verified against current docs)

| Engine | Component lifecycle | Editor vs play | What we take |
|---|---|---|---|
| Unreal | `OnRegister`/`OnUnregister` (component registered with the world, also in the editor); `OnConstruction` (actor construction script, re-runs on property change in the editor); `BeginPlay`/`EndPlay(Reason)` (play only); `TickComponent` in tick groups with prerequisites. | `GetWorld()->IsGameWorld()` / `EWorldType`. | `construct` = OnConstruction/OnRegister (both modes), the ordering "construct before BeginPlay", tick groups (our system labels, already modeled on them, `systems.rs:20-24`), `EndPlay` reason (we may add a reason later). |
| Unity | `Awake`/`OnEnable`/`Start`/`Update`/`OnDisable`/`OnDestroy`; `OnValidate` on inspector change in the editor; `[ExecuteAlways]`/`[ExecuteInEditMode]` opt-in to editor run. | `Application.isPlaying`. | `construct` ~ `OnValidate` (editor edits, coalesced by the editor); `SessionKind` ~ `isPlaying`; the opt-in idea for editor-run `tick` (not allowed in this plan). Unity's `OnDestroy` can read the object because destruction is deferred to frame end; relevant to D2. |
| Godot | `_enter_tree`, `_ready` (once, after children), `_process`/`_physics_process`, `_exit_tree`; `@tool` scripts run in the editor. | `Engine.is_editor_hint()`. | `SessionKind` ~ `is_editor_hint`; `_ready` after the subtree exists ~ "begin_play after the whole level is present"; `_exit_tree` is called with the node still intact, again relevant to D2. |
| Bevy | schedules (`Startup`, `Update`, `FixedUpdate`), `States`/`OnEnter`/`OnExit`, component hooks (`on_add`, `on_insert`, `on_remove` with the value still readable) and observers. | none built in; states model it. | systems + labels (already ours); `OnEnter`/`OnExit` ~ session begin/end; `on_remove` hooks run before the value is dropped, which is the cheap way to give `end_play` data (D2 alternative). |


## 4. Decisions (all **Proposed, needs the user's confirmation**)

**D1. Name of the new lifecycle trait.**
Recommend: **`Actor`**, in its own crate/module path, not re-exported from the
`pulsar_game` prelude until SceneDB's is removed (P5), then added there.
Why: it is the user's stated wish and matches Unreal's vocabulary. Cost found in
this audit: there are already two `Actor`s (SceneDB's, and Helio's render trait
at `helio-core/src/actor.rs:3`, re-exported at `helio/src/lib.rs:22`).
Anything glob-importing both `helio::*` and the prelude would collide after P5.
Alternatives: `ComponentActor` (no collision, longer), or `Lifecycle`. If you
prefer a collision-free name, rename Helio's instead (it is render-only and has no
user in this repo: no `impl` of it found, UNVERIFIED for external code).

**D2. Does `end_play` get component data?**
Recommend: **stay entity-only first**, with the session-end teardown run
**before** the snapshot restore (2.3). Why: the framework cannot hand out removed
data (journal has no payload, finding 9) without either (i) deferred removal
(components removed only after `end_play` runs, which changes SceneDB semantics and
undo/snapshot assumptions) or (ii) copy-out (clone every component with an
`end_play` before it can be removed: memory and `Clone` cost on every class).
Today's classes (voxel, skm_editor) live with entity-only. Revisit only if a
class needs its own data at end; the Bevy `on_remove`-style hook is the likely
mechanism and would be a SceneDB change.

**D3. Hard break or deprecation window for `impl pulsar_scenedb::Actor`.**
Recommend: **a deprecation window of one engine release, only for hand-written
native script crates; generated Blueprint classes break hard (regenerated).**
Why: generated classes and `engine_main` are regenerated on every Blueprint
compile (`core_project_builder.rs:1150`, "Generated. Do not hand-edit",
`pulsar_script_codegen/src/actor.rs:228,321`) and the rev is baked from the root
manifest with stale entries replaced (`engine_backend/build.rs:22`,
`core_project_builder.rs:355-360`), so they follow the engine. Hand-written crates
(`Spinner` template, `core_project_builder.rs:908-921`, plus users') compile
against `pulsar_game::{Actor, ...}` (`pulsar_game/src/lib.rs:20`). Window
mechanism: for one release `pulsar_game` re-exports SceneDB's trait **and**
`register_actor` keeps accepting it (wrapped into a lifecycle-registered shell)
with `#[deprecated]`; P5 removes it. If you want no shim, the break is limited to
those crates and the Spinner template is updated in the same change.

**D4. `construct` coalescing.**
Recommend: **at most once per frame per component instance, delivered at one
fixed pump point after the frame's writes, tagged with the set of changed
property names when known; a write made *by* a `construct` does not re-trigger
construct for that instance.** Slider drags (construct storms) cost one call per
frame, not one per mouse event. Restores/undo/level load are treated as one
rescan, not N changes. Open sub-choice: whether `construct` is rate-limited
further (e.g. after the drag ends) for expensive classes; recommend a per-class
opt-in `construct_debounce` later, not in P2.

**D5. `begin_play` order for level-load components.**
Recommend: **undefined by contract and order-insensitive**, deterministic in
practice (class `type_name`, then entity bits; today's order,
`component_lifecycle.rs:221-222,288`). Why: `ClassSpec::requires` is attachment,
not ordering (`class_spec.rs:11-17`), and a begin order that depends on level
file order would make saves non-portable. A component that needs a sibling calls
into it lazily or uses a `System`. If ordering is ever needed, add an explicit
`after`/`before` on the `ActorTable` (like `System`, `systems.rs:49-55`) rather
than relying on `requires`.

**D6. Where the capability registry crate lives.**
Recommend: **a new crate `crates/core/pulsar_component_lifecycle`** (depends on
`pulsar_scenedb`, `pulsar_events`, `pulsar_plugins`; **not** on `pulsar_reflection`
or `pulsar_script_vm`), with `pulsar_world_registry` depending on it for the
orchestrator. Why: it makes the "unreachable from Blueprints" guarantee a crate
graph property enforceable by `just arch`, not only a naming test (2.6).
Cost: the new crate needs its own runtime-attach plumbing for plugin DLLs
(`runtime.rs` pattern, `WorldRuntimes` gains a field, `ABI_VERSION` bump).
Fallback if that cost is too high: a `lifecycle` module inside
`pulsar_world_registry` (the audit test then carries the whole guarantee).

**D7. `SessionKind` for the guest dylib.** Recommend the host passes the kind
across the PIE ABI (`pulsar_pie_abi` context) rather than the guest guessing; a
new field means an ABI version bump of the pie ABI (UNVERIFIED how
`pulsar_pie_abi` versions fields, `lib.rs:360` area).

## 5. Phases

Common rules: Pulsar-Native PRs only touch Pulsar-Native unless a row says
otherwise. Submodule order follows `.agents/CRATES.md` "Submodule pins"
(`:316-330`): dependency PR merged into its upstream main, submodule re-pinned,
then the Pulsar PR. Cross-checks to run in every phase: `just arch`,
`just check-submodule-pins`, `cargo test -p pulsar_world_registry -p pulsar_game`.

### P1. Capability registry + `Actor` trait, legacy adapter, leak audit

- **Scope**: the L1 crate (D6); `ActorTable`, `ActorRegistration`, lookup;
  `ClassSpec::actor`; `Components::class` registers it; the orchestrator in
  `tick_live_components` drives both new tables and legacy
  `ComponentTickRegistration` in one `Step::Components`; ABI 3 -> 4; docs.
  Per-instance and world forms (2.5) defined; **no** `construct` event delivery
  and **no** session kind yet (the fn exists, unused).
- **Files/crates**: new `crates/core/pulsar_component_lifecycle`;
  `pulsar_world_registry/src/{lib,runtime,component_lifecycle,audit}.rs`;
  `engine_backend/src/scene/class_spec.rs`; `architecture/layers.toml`; root
  `Cargo.toml` members; `.agents/CRATES.md`, `.agents/ECS.md`.
- **Repos**: Pulsar-Native only.
- **Exit criteria / tests**:
  1. a test class with an `ActorTable` gets begin_play (first frame), tick per
     frame, end_play on remove/disable/despawn in the same order as the existing
     `component_lifecycle.rs` test;
  2. legacy classes unchanged: `component_event_routing.rs`, `character.rs`,
     skm_editor and voxel suites pass untouched;
  3. **leak audit** (2.6) green over the plugin-aware test crate;
  4. a class registered in both paths is reported as an error;
  5. a disabled plugin's table is not returned (gate), mirroring `class_enabled`;
  6. the `ABI_VERSION` fingerprint test covers the new type;
  7. `just arch` rule for L1/L2 edges.
- **Parallel**: nothing in P1 is parallel with another code phase; the doc and the
  Spinner template notice (D3) can be written alongside.

**As built (P1).** New crate `crates/core/pulsar_component_lifecycle` (kernel
layer; deps: `pulsar_scenedb`, `pulsar_events`, `pulsar_plugins`): the `Actor`
(`&mut self`, no world) and `WorldActor` (`&mut World` + instance entity) traits,
`ActorCx { entity, instance, events }`, `ActorTable` (`of::<T>()`, `of_world::<T>()`,
public fields to mix forms per event), `ActorRegistration` (category
`component.lifecycle`, `pulsar_plugins::importer!`), `ActorIndex` (sorted by
component id for binary-search lookup, by type name for the frame order) and
`RESERVED_METHODS`. `ClassSpec::actor` and `Components::class`
(`engine_backend/src/scene/class_spec.rs`, which also re-exports the traits)
add the registration. `pulsar_world_registry`: `runtime.rs` carries the actors
list (`ACTORS`, gated by `class_enabled`, imported with plugin builds, handed
across DLLs); `ABI_VERSION` 3 -> 4, the fingerprint now hashes a named `LAYOUTS`
table that includes `ActorRegistration` and `ActorTable`; `component_lifecycle.rs`
drives legacy `ComponentTickRegistration` and tables through one `Lifecycle`
enum in one `Step::Components` (deterministic order by type name; removals,
`end_all` and the set-difference end path are shared); `lifecycle_conflicts()`
reports a class in both paths (logged once per session; the legacy one runs);
`audit.rs` adds `find_lifecycle_problems`/`lifecycle_leaks`/`LifecycleAuditError`
(reflected methods, `Class::<name>` natives, double registration). The catalog
source `component.lifecycle` is submitted from `component_lifecycle.rs`.
Helio: `helio-core/src/actor.rs` and its re-exports deleted (submodule branch
`feature/component-lifecycle`, not pushed). Docs: `CRATES.md`, `ECS.md`,
`PLUGIN_SYSTEM.md`, `layers.toml` (explicit crate entry plus comment).
Tests: `pulsar_component_lifecycle` (`lookup_finds_by_component_id_and_orders_by_type_name`,
`crate_edges`); `pulsar_world_registry` (`component_lifecycle::tests::actor::
a_table_class_gets_begin_tick_and_end_in_order`, `a_class_in_both_paths_is_a_conflict`,
`runtime::tests::the_fingerprint_covers_the_lifecycle_types`,
`audit::tests::a_reserved_name_is_a_leak`, `this_binarys_registry_is_isolated`);
`plugins/installed/tests/lifecycle.rs` over a new fixture plugin
(`tests/fixtures/lifecycle_plugin`, `pulsar.test_lifecycle`): order on
disable/removal/despawn, the plugin gate and catalog, and the leak audit over
every installed class.

Deviations: (1) `end_play` is entity-only in both forms in P1 (no `&mut World`):
`end_all` and `process_component_removals(&World)` have no mutable world;
P2's session end must pass one if the world form needs it at end. (2) No deferred
`commands` on `ActorCx` yet (2.5); `events` is the hub directly. (3)
`layers.toml` is layer-based and cannot express a crate-pair rule, so the
"must not depend on reflection/script VM" edge is enforced by the
`crate_edges` test (direct deps only: SceneDB itself depends on
`pulsar_reflection`), and `script_natives` has no edge to the new crate's
tables (it never names them). (4) The gate test uses a real fixture plugin
rather than the catalog fixture, so no other test binary sees the new class.
(5) The ABI-fingerprint test checks that the new types are in the named layout
table and the version is 4. `construct` is declared on both traits and in the
table, never delivered. For P2: the orchestrator is
`tick_actor_class`/`tick_component_lifecycles` in `component_lifecycle.rs`;
`ComponentRuntimeState` is still the session; a future `Session` should replace
the unused-by-actors `subscribe_instance` bookkeeping only if handler events
are added to tables.

### P2. `construct`, the editor pump, the session scope

- **Scope**: `Session` (kind, resources, settings, end guarantee) evolving
  `ComponentRuntimeState` (alias kept); `construct` delivery for editor and play,
  coalesced (D4); world ownership rule (2.4); explicit `end_session` ordering in
  `PieSession::shutdown`/`pie_shutdown` and `run_blocking`, `TickLoop::drop`;
  host passes `SessionKind` (D7); `GameSessionRegistration::end` unchanged.
- **Files/crates**: the L1 crate; `pulsar_world_registry/src/component_lifecycle.rs`;
  `pulsar_game/src/{tick,embed,standalone}.rs`; `pulsar_pie_abi`; the editor pump
  call site in `ui_level_editor`/`engine_backend` (**UNVERIFIED location**; the
  candidate is the once-per-frame `end_change_window`,
  `SCENEDB_MIGRATION.md` "Observers"); `engine_backend/src/services/pie_host.rs`.
- **Repos**: Pulsar-Native only.
- **Exit criteria / tests**: (1) `end_play` runs for every live instance **before**
  `exit_play_mode`, tested with the PIE flow pattern in
  `ui_level_editor/src/core/scene_edit/tests/classes.rs:716-763`; (2) 100 writes
  to one component in one frame produce 1 `construct`; (3) a write inside
  `construct` does not re-enter it; (4) snapshot restore produces one rescan, not
  N constructs; (5) `kind()` is correct in Editor, PIE, headless
  (`standalone.rs:384` path), standalone; (6) the editor pump is suspended while a
  play session owns the world and resumes after Stop; (7) `scripts_only` reload and
  a no-op "Play again" fire no `begin_play`.
- **Parallel**: needs P1. Can run in parallel with P5's SceneDB-side prerequisite
  (see P5) because that is an external repo.

### P3. Migrate physics

- **Scope**: body/collider/joint/force-field/character `begin_play`/`end_play`
  (world form) into a session resource; remove `SESSIONS`, `key_of`,
  `IDLE_SESSION`, the prune; sim shutdown with the last component; read gravity
  and `physics_enabled` from session settings; `simulated_counts`/`wait_idle` on
  the session. The step stays a `System`.
- **Files/crates**: `plugins/vendor/physics_plugin/src/{runtime,registrations,sim,behavior,lib}.rs`
  and its components; tests (`runtime.rs:471-780`).
- **Repos / merge order**: `Plugin_Physics` (the physics submodule) and
  Pulsar-Native. The physics crates depend on Pulsar's APIs, so: merge P1+P2 in
  Pulsar-Native main first; develop the physics PR with the submodule pinned to the
  feature branch in a Pulsar PR (allowed in review, `CRATES.md:327-328`); merge the
  physics PR into `Plugin_Physics` main; re-pin; merge the Pulsar PR. (Reverse of
  the usual "dependency first" because here the plugin is the dependent;
  **UNVERIFIED** that this is how plugin API changes have been sequenced before.)
- **Exit criteria / tests**: the existing runtime tests pass; new tests: gravity
  from level settings changes motion; `physics_enabled = false` registers nothing;
  two sessions in one process do not share a sim; Stop joins the sim thread;
  removing the last body shuts the thread down and the next one restarts it; no
  global `SESSIONS` left (grep).
- **Parallel**: with P4 once P2 is merged (different repos and crates).

### P4. Blueprint class-instance lifecycle through the component

- **Scope**: `ClassInstance` implements `Actor` natively (2.8); the script driver
  becomes a session resource; delete its own cursors/`rescan`/`sync_entity`/
  `tracked`/`holders` and keep globals, roles, possession, class reload.
  Generated Rust Blueprint actors still `impl` the old SceneDB trait here.
- **Files/crates**: `pulsar_class`, `pulsar_game/src/scripting/driver/*.rs`,
  `pulsar_game/src/tick.rs`; `pulsar_script_runtime` only if spawn/begin_play
  hooks need a seam.
- **Repos**: Pulsar-Native only (the Blueprint plugin `plugins/vendor/blueprint_editor`
  is only touched if it registers class-instance behavior; UNVERIFIED).
- **Exit criteria / tests**: the whole `pulsar_game/src/scripting/tests.rs` suite
  passes unchanged in behavior; session end gives `end_play` to every Blueprint
  instance before the PIE restore; a class change on a live object restarts it
  once; disabling a `ClassInstance` ends its script; `scripts_only` reload keeps
  `has_begun` instances and re-fires nothing.
- **Parallel**: with P3; both need P2.

### P5. Remove `Actor`/`ActorRegistry` from SceneDB

- **Scope**: external repo `Far-Beyond-Pulsar/SceneDB` first (remove
  `actor.rs`, the root re-export, docs, tests), then bump the rev everywhere,
  then repoint consumers: `pulsar_game` (`tick.rs` `actors` field and phase 2,
  `scripts.rs` `ReboundActor`/`register_actor`, `lib.rs:20,173`, tests,
  `export_probe.rs`, `blueprint_codegen_drift.rs`, `build.rs`),
  `pulsar_script_codegen/src/actor.rs` (emit the new trait or a component that
  registers one), `core_project_builder.rs` (`Spinner` template, `engine_main`
  docs), `generated_project_compiles.rs:252`; docs `ECS.md`, `CRATES.md`,
  `blueprint-system.md`. Hot-reload rebinding (#653) must be re-expressed on the
  lifecycle (a native script actor is a component instance; the `ScriptTag` +
  rebind logic maps to "same class on the same entity re-attaches").
  The Helio re-export (`helio/src/lib.rs:22`) is **unaffected** (finding 1).
- **Repos / merge order**: (1) SceneDB PR merged to its main; (2) one Pulsar PR
  bumping **three** manifests: root `Cargo.toml:406-407` and `:807-808`, Helio
  `Cargo.toml:64-65` (Helio is a submodule: Helio PR merged to Helio main first,
  then re-pinned); (3) the repoint changes in the same Pulsar PR (the old and new
  SceneDB revs cannot coexist, the `[patch]` forces one rev, `Cargo.toml:800-806`).
  Plugins carry no rev (finding 2).
- **Exit criteria / tests**: drift guards re-aimed at the new trait and still
  failing on drift; `cargo test -p pulsar_game` incl. `generated_project_compiles`;
  a generated project builds against the baked rev; the deprecation shim (D3)
  removed after its window; no `pulsar_scenedb::Actor` text left (grep).
- **Parallel**: the **SceneDB-side change** can be prepared in parallel with P1-P4
  (it only deletes code the engine would no longer use) but cannot merge before
  P4 and the shim decision land, since `pulsar_game` still uses it until then.

### P6. Retire the raw `inventory` lifecycle registration

- **Scope**: `#[register_component_runtime]` emits an `ActorTable` registration
  (class-level `ClassSpec::actor`, or a macro-submitted registration with an
  origin) and the legacy shim path is deleted: `ComponentTickRegistration`,
  its `TICKS` list, `Runtime::ticks`, the fingerprint term (`runtime.rs:37,75,85,95,122,152,255`),
  `ComponentInstanceKey`'s legacy users, the macro's `previous/current` shim
  (`component_runtime.rs:335-369`). Migrate voxel and skm_editor
  (`component_runtime.rs:16`, `component.rs:1574`), the engine test
  (`component_event_routing.rs:34`) and `ComponentRuntimeState::default()` test
  users (alias removed). ABI bump.
- **Repos / merge order**: Pulsar-Native (derive + registry) merged first with
  the macro still accepting today's source syntax (the macro keeps its input
  shape, only its expansion changes), then **no** plugin source change is
  required; plugin repos only re-pin. If the macro syntax changes (it should
  not), voxel and skm_editor PRs go first.
- **Exit criteria / tests**: grep shows no `ComponentTickRegistration`; all
  plugin suites pass; the leak audit still green; the ABI fingerprint test.
- **Parallel**: after P1; independent of P3-P5 except the shared derive file and
  the ABI version (serialize ABI bumps: P1 -> 4, P2/P6 -> 5, 6).

## 6. Risks

| Risk | Detail | Mitigation |
|---|---|---|
| **`end_play` after removal cannot see the component** | Journal has no payload (finding 9). Class code that cleans up by reading its own data will not work. | D2: entity-only first, session-end teardown before restore; document that cleanup state belongs in session resources keyed by owner; revisit with an `on_remove`-style hook later. |
| **External SceneDB repo coordination and the triple rev pin** | P5 spans SceneDB, Helio and Pulsar; the `[patch]` makes mixed revs impossible (`Cargo.toml:800-806`); `check-submodule-pins` requires Helio's pin on its main (`CRATES.md:316-330`). | Prepare the SceneDB PR early; one Pulsar PR bumps all pins; verify with `cargo tree -d` that one rev remains. |
| **User game crates compile against `pulsar_scenedb::Actor`** | `pulsar_game` re-exports it (`lib.rs:20,173`); the template teaches it (`core_project_builder.rs:908-921`). | D3: deprecation window for hand-written crates; generated code regenerates; ship a migration note and update the template first. |
| **Hot reload and `scripts_only` Play must not re-fire `begin_play`** | `scripts_only` keeps the host (`game_viewport.rs:389-401`); the dylib swap today fires `end_play` then `begin_play` for scripts and components (new session on the same world). | Lifecycle state keyed by instance + class generation; test (P2 exit 7). Define the swap explicitly as end-session + new-session; native actor rebinding (#653) stays a P5 concern. |
| **Callback re-entrancy** | A per-instance callback holds the component row borrowed from the world (`lib.rs:237-240`); physics-style classes want sibling access. | Two forms (2.5): no-world per-instance form, and a world form that takes no component borrow; commands deferred; audit that world-form callbacks never run inside a query iteration. |
| **Construct storms** | A slider drag writes a property every mouse event; every write is a journal entry. | D4: one call per frame per instance, no self-retrigger; restore/undo = one rescan; measure with a 1000-instance scene. |
| **Editor/PIE share one world** | Both pumps would fire `construct` and the play session ends while the editor still holds the world (finding 4). | World ownership rule (2.4); order in 2.3; tests P2. |
| **`begin_play` of a level-load component before the world is ready** | SceneDB's `register` called it at registration time (`actor.rs`); the engine deliberately defers (`tick.rs:504-511`). | Session-ready gating in P2; keep the deferral. |
| **Three `Actor` names** | SceneDB, Helio, new (finding 1). | D1; path-qualified use until P5; do not add to the prelude early. |
| **Plugin DLL ABI** | New registration types change `Runtime`/`WorldRuntimes` layout; fingerprint mismatch refuses attach (`runtime.rs:28-53`). | Bump once per phase that changes layout (P1, P2, P6); the fingerprint test. |
| **Physics thread lifetime during PIE restore** | Today the thread outlives the game until a later tick prunes it (`runtime.rs:258`) or the world address is reused (`:234-237`). | Session resource drop joins it (P3 exit: Stop joins). |
| **Behavior change: Blueprint `end_play` now runs at session end** | Finding 3. | Release note; test; it matches the documented contract. |
| **`inventory` and `Gated` caching** | Gated lists are rebuilt when plugins/registrations change and leak one list per rebuild (`runtime.rs:185-225`). | Reuse the same gate type for the new registry; no new leak pattern. |

## 7. Unverified or not read

- **Submodules are not initialized in this worktree** (`crates/renderer/helio`,
  `plugins/vendor/*` are empty here). The physics, voxel, skm_editor and Helio
  files were read read-only from the main checkout at
  `C:\Users\redst\OneDrive\Documents\GitHub\Pulsar-Native`, which is at the same
  commit (`d100b68a3`; physics pin `714440839e`, voxel `6a799ee5`, skm_editor
  `3d6698b9`, Helio `c595d1fe`). Their `file:line` references are to those files.
- Whether anything **outside** this repo `impl`s Helio's `Actor` (finding 1, D1).
- The **editor pump call site** for `construct` (P2): `end_change_window` is the
  candidate from `SCENEDB_MIGRATION.md`, not read in code here.
- How `architecture/layers.toml` expresses crate-to-crate edges (2.6); not read.
- `pulsar_pie_abi` field versioning for `SessionKind` (D7); only the symbol list
  at `lib.rs:360` was seen.
- Whether physics' `simulated_counts`/`wait_idle` have callers outside the
  physics crates (P3).
- Whether the Blueprint plugin (`plugins/vendor/blueprint_editor`) registers class
  instance behavior that P4 would have to touch.
- Whether world settings can change live in PIE (affects "read at begin_play"
  for gravity, 2.7).
- How plugin PRs that need new Pulsar APIs have been sequenced in practice (P3).
- The `Standalone` windowed path (`run_with_windows`, `tick.rs:711-825`) was only
  skimmed for session end; `run_blocking` and `Drop` were read.
- Did not read the SceneDB repo's own consumers of `Actor` (its tests/examples),
  which the P5 SceneDB PR must also update.
