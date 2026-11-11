# Traits

A **trait** is a named contract: a set of methods an implementer promises to
provide. Traits are language-agnostic data (`*.trait.json`): the project's own,
and those the engine and the enabled plugins serve read-only into every
project. Implementers come from registered **sources**: Blueprints (the
Blueprint plugin) and native components (from reflection), and any plugin can
add its own kind. The engine uses traits to give gameplay **roles** to classes
(game mode, player controller, ...), and the project's trait index answers
"who implements this?". The one owner of all of it is `pulsar_traits`.

## Pieces

| Piece | Where | What |
|---|---|---|
| Trait asset | `ui_types_common::TraitAsset` (`types.rs`); editor in `plugins/vendor/trait_editor` (the `pulsar.trait_editor` plugin) | `.trait.json`: `id`, `name`, `methods`, `supertraits`, `meta` (`contractVersion`) |
| Records | `pulsar_traits::records` (`TraitRecord`) | Tolerant parse of a trait asset, no `ui_types_common` needed (runtime-safe) |
| Engine traits | `pulsar_traits::builtin`, files in `crates/core/pulsar_traits/content/traits/` | `IntoGraph` and the four role traits, provided into every project at `plugins/pulsar/traits/` (the engine's plugin folder) |
| `TraitDb` | `pulsar_traits::db` | Resolved in-memory model: maps, supertrait closure, effective implementers, each implementer's source kind |
| Sources | `pulsar_traits::sources` | `TraitImplementerSource` (kind, file scan, listed implementers), `Binding` (how the runtime reaches an implementer), `NativeTraitImpl` + `implements!` (native components) |
| Index | `pulsar_traits::TraitIndex` (`index.rs`) | The project's one index over every source and the mounted traits; stores `.pulsar/trait_index.json` (schema 3), incremental `update_path` |
| Conformance | `pulsar_traits::conformance` | Does an implementer provide the methods of the traits it declares? |
| Roles | `pulsar_traits::roles` | `RoleRegistration` (the registry), `RoleSelection` (level -> paths), `WorldRoles` (resolved + checked, plus the level's settings) |
| Blueprint source | `plugins/vendor/blueprint_editor/src/features/traits/source.rs` | Which files are Blueprints, what they declare and provide, `check_blueprint`; registered by the plugin's root |
| Reflection | `pulsar_reflection::trait_registry` (`TRAIT_REGISTRY`) | Published, language-agnostic snapshot per project |
| Plugin API | `plugin_editor_api::ProjectTraits` | `traits()`, `implementers(trait)`, `traits_of(asset)`, `implements`, `to_json` |
| Blueprint editor | `plugins/vendor/blueprint_editor/src/features/traits/` | Trait panel, Add-menu trait events, conformance at compile/preflight |
| Runtime | `pulsar_game` (`roles.rs`, `scripting/driver/roles.rs`, `scripting/traits.rs`) | Spawns the selected role classes as session entities, answers "does this entity implement trait X", dispatches trait methods to script handlers and native component methods (`RuntimeTraits`, the one dispatcher), publishes the role lifecycle on the hub |

## Identity: ids, not paths

A trait's identity is its **id**; the path is where it currently lives.

- The id is embedded in the asset (`"id"`). The trait editor keeps it and mints
  one for a new trait. The index assigns one to any project trait that has none
  when it is rebuilt (the file is rewritten once). If the file cannot be
  written, the id comes from the repo's durable asset-id manifest
  (`.pulsar/asset_ids.json`, kind `trait`, via `ProjectFileIndex::ensure_typed_asset_id`,
  which also follows file moves), and as a last resort it is `path:<path>`
  (not rename-safe; logged). Ids are also mirrored into the file index
  (`record_typed_asset_id("trait", ...)`) so pickers can look traits up by kind.
- A copied trait file carries the same id; the copy that sorts second gets a
  fresh id. A copy of a mounted trait becomes an ordinary project trait.
- **Namespaces.** A mounted trait's id is its owner's: `pulsar.<Trait>` in the
  engine's mount, `<plugin id>.<Trait>` in a plugin's (`pulsar.lights.Glows`),
  one segment after the owner, with a `meta.contractVersion` (>= 1). A mounted
  trait outside its namespace or without a version is refused and reported
  (`TraitIndex::problems`). A **project** trait whose id is in a reserved
  namespace (the engine's or an installed plugin's) is refused too: an old
  project's copy of a built-in (`types/traits/game_mode.trait.json` from before
  p21) is reported, left as it is, and the mounted original serves instead.
- Implementers store ids (`blueprint_metadata.implemented_trait_ids` for a
  Blueprint) and may keep paths (the pre-id format, a fallback and a hint).
  Resolution: ids first; then paths. A path beside ids that all resolve is only
  a stale hint after a rename and is ignored. An implementer with paths but no
  ids is **path only** (`ImplementerEntry::path_only`): it still works, and its
  editor adds ids when it next saves. Nothing rewrites an implementer behind
  your back.
- Renaming or moving a trait therefore keeps its implementers. References
  that name nothing are kept as `unresolved`, shown as "Missing trait", and
  never dropped silently. Disabling a plugin removes its mount, so its traits'
  implementers see "Missing trait" the same way.
- Supertrait references (`supertraits`: ids or paths) resolve the same way.

## Implementer sources

A `pulsar_traits::TraitImplementerSource` (an origin-gated contribution,
category `traits.implementer_sources`) tells the index how to find one kind of
implementer:

- `kind`: its id, prefixed by its owner (`pulsar.blueprints.blueprint`); what
  `TraitImplementer::kind` says;
- `claims` + `scan`: which project files it reads, and one file's implementer
  (`Scanned { id, declared, binding }`);
- `list`: implementers that are not files, listed whenever the index loads.

Each implementer carries a **binding**, how the runtime reaches it:
`Binding::ScriptClass { dir }` (a compiled class: a trait method is its handler
for the event `<Class>.<method>`) or `Binding::Component { class }` (a native
component: a trait method is the component method it is bound to).

- **Blueprints**: the Blueprint plugin's root registers the source
  (`features/traits/source.rs`). A class's `graph_save.json` binds to its class
  folder, so the runtime meets Blueprints through the index and compiled
  classes, never a Blueprint file. A loose `*.blueprint.json` is indexed with no
  binding. The plugin is an editor tool: a game links none of this.
- **Native components**: `pulsar_traits` registers the source over every enabled
  `NativeTraitImpl`, which a plugin's root submits:

  ```rust
  pulsar_traits::implements!(HealthComponent: "pulsar.health.Respawnable");
  pulsar_traits::implements!(HealthComponent: "pulsar.health.Damageable" {
      take_damage => apply_damage,
  });
  ```

  Each trait method binds to one of the component's registered methods
  (`pulsar_reflection::ComponentMethodRegistration`, the by-name methods scripts
  and the editor call), the same name unless mapped, so `health.respawn()` and
  the dispatcher calling `Respawnable.respawn` run the same function. The
  binding is checked by the conformance rules with `ProvidedMethod`s built from
  `MethodMetadata` (`sources::native_conformance`); a mismatch is an index
  problem. SceneDB knows nothing about traits.

A plugin's own asset kind (a behaviour tree, a state machine) registers a
source and gets indexing, conformance and dispatch with no other change.

## The index

`TraitIndex` is derived data. `rebuild` scans the project (opening a project
does it: providers have no reliable freshness contract), `update_path(root,
path)` re-reads **one** implementer or trait file (save hooks use it), `load`
reads the stored index, and `live(root)` is what the engine and plugins bring
alone (a game whose content ships no index).

What is **stored** (`.pulsar/trait_index.json`, schema 3) is only what the
project's own files say: its traits, its file implementers, and the problems
with them. Everything the engine and plugins bring (mounted traits and
implementers, listed native implementers) is read again whenever the index
loads, so it always matches the enabled set. Persisted output is sorted and
deterministic.

Lookups are maps (`TraitDb`): `implementers_of(id | path | name)`,
`traits_for(implementer)`, `resolve_trait`, `entry_for_binding`.
**Supertraits are modelled**: an implementer that declares `MyMode: GameMode`
is an implementer of `GameMode` (`Implementation::via` names the declared
trait). Cycles are detected and do not hang. The type system's
`type-index/index.json` is cross-referenced (`TraitRecord::type_index_id`).

## Engine traits

Engine-owned, defined as files (`crates/core/pulsar_traits/content/traits/`),
embedded in the engine and provided at `plugins/pulsar/traits/` in every project
(the engine's `FileProvider`, the same mechanism a plugin uses), with fixed
ids (`pulsar.GameMode`, ...). A project cannot change them (a write fails with
`engine_fs::ReadOnlyMount`; the trait editor opens them read-only). To change a
contract, edit the file and bump its `meta.contractVersion`.

| Role | Trait | Methods |
|---|---|---|
| Game Mode | `GameMode` | `on_game_start()`, `on_player_joined(player_id: u32)`, `on_game_end()` |
| Game State | `GameState` | `on_init()`, `on_player_added(player_id: u32)`, `on_player_removed(player_id: u32)` |
| Player Controller | `PlayerController` | `on_possess(pawn_id: u64)`, `on_unpossess()`, `on_input(delta_seconds: f32)` |
| Player Character | `PlayerCharacter` | `on_spawned()`, `on_possessed(controller_id: u64)`, `on_unpossessed()` |

Ids are plain integers until the spawn path hands Blueprints typed actor
references (a parameter type change is a contract version bump).

## Roles

A role is a `pulsar_traits::RoleRegistration` (an origin-gated contribution,
category `gameplay.roles`): `id` (`pulsar.game_mode`), `label`, `trait_id`,
`setting_key` (the key in the level's `world_settings`), `order` and `start`
(`Now`: spawned and started in the session's first script phase;
`EndOfFrame`: queued like a script's spawn). The engine registers its four
(`pulsar_traits::builtin`: `game_mode_blueprint`, `game_state_blueprint`,
`player_controller_blueprint`, `player_character_blueprint`, the keys levels
always used); a plugin registers its own from its root, and a disabled plugin's
roles are absent. Adding a role is one registration and a trait.

World Settings builds one picker per registered role, listing the implementers
of its trait that run as a script class (effective, so sub-traits count). The
selections live in `WorldSettingsData::values` under each role's key, with
every plugin's field (`ui_level_editor::extend::WorldSettingField`: the input
plugin's player input mapping, the physics plugin's toggle and gravity),
rendered by kind; keys nobody registers now round-trip untouched.

`WorldRoles::for_level(level_json, project_root)` resolves a level's selections
against the stored index (read-only; `Unchecked` when there is none, as in
shipped content): `Valid`, `NotInIndex`, `MissingTrait`, `Unchecked`. It also
carries the level's `world_settings` (`WorldRoles::setting(key)`), where a
plugin reads its own (the input plugin's `PLAYER_INPUT_MAPPING_KEY`, through
`game_hooks::PossessionContext::roles`). `TickLoop::load_world_roles` is called
when a level loads, in standalone (`standalone::load_level`) and Play-in-Editor
(`embed.rs`, from `target/pie/play.level`).

How others do it: Unreal's `AGameModeBase` names its classes
(`PlayerControllerClass`, `DefaultPawnClass`, `GameStateClass`), each a
`TSubclassOf` the World Settings override per level; a plugin adds a slot by
subclassing the game mode. Here the slots are data: a plugin adds a role by
registering it, and the slot names a trait rather than a base class, so any
implementer kind fills it.

## Trait methods are event nodes

When a Blueprint declares a trait, each required method (supertraits included)
appears in the node Add menu under `Events/Trait/<Trait>` as an `On <Class>.<method>`
entry point (an ordinary engine event node, `event::on::<Class>.<method>`); its
parameters are the node's outputs. The entries are derived from the trait ids
and the project's traits (`features/traits/events.rs`), never stored: removing
the trait removes them, and a renamed trait still resolves. At compile (and in
preflight) the events are declared on the class so the engine can call them.
Placing the node is what *provides* the method for conformance.

- A trait method's **return type** has no counterpart in the event system. It is
  shown in the node description; a handler cannot return a value, and
  conformance does not judge return types of event-node implementations.
- **Removing a trait does not delete placed nodes.** They become *orphaned*:
  the editor reports the count, and the Blueprint does not compile until they
  are deleted or the trait is added back (which revives them, the node type is
  unchanged).

## Conformance

`pulsar_traits::conformance::check(db, declared_ids, provided, severity)` is
the one rule; each source builds the `ProvidedMethod`s from its own format. For
Blueprints, `check_blueprint(db, blueprint_json)` (the Blueprint plugin's
`features/traits/source.rs`) checks every declared trait. A method is provided by a custom event (`local_events`), a macro
(`subgraphs`), or a placed trait event node, matched by snake-cased name. Missing
methods (without a default body), wrong parameter counts and incompatible types are
errors. It is deliberately conservative: parameter names are not compared, numeric
types are one class, and anything uncomparable (generics, tuples, unrecorded macro
pin types) matches. **Legacy**: a path-only Blueprint gets the same findings as
*warnings* until it is saved with ids, so an old project does not stop building
the day this lands. The compiler (`compile_to_rust`) and preflight
(`validate_project_classes`) enforce it.

## Plugins

Trait data reaches plugins through the reflection registry; there is **no new
plugin vtable or `EditorServices` field**, so `PLUGIN_ABI_REVISION` is unchanged. A plugin's
copy of `pulsar_reflection` attaches to the editor's runtime (see
`PLUGIN_SYSTEM.md`) and reads what the host published. The reflection `Runtime`
table gained a `traits` entry, so its `ABI_VERSION` is 3: a plugin built against
older reflection is refused at attach, like any runtime change.

```rust
let traits = plugin_editor_api::ProjectTraits::for_project(&project_root);
traits.traits();                       // Vec<TraitDescriptor>
traits.implementers("pulsar.GameMode"); // id, path or unique name
traits.traits_of("classes/Hero/graph_save.json");
traits.to_json();                      // for a non-Rust plugin
```

The host publishes after every index rebuild/update/load
(`TraitIndex::publish`). Snapshots are per project (`pulsar_traits::scope_key`).
`TraitImplementer::kind` is the implementer's source kind
(`pulsar.blueprints.blueprint`, `pulsar.native.component`, a plugin's own).

A plugin brings traits by providing them: `.trait.json` files in its plugin
folder (its `engine_fs::FileProvider`, from any source, embedded or generated)
appear at `plugins/<plugin id>/` in every project that enables it, namespaced
`<plugin id>.<Trait>` and versioned
(`meta.contractVersion`). It implements traits with a `TraitImplementerSource`
(its own asset kind) or `implements!` (its components). Depending on another
plugin's trait is a `requires` entry (p22, #1608 phase 3).

## Editing a Blueprint's traits (no hitches)

Adding/removing a trait is a metadata edit. The Add-menu entries are spliced into
the profile catalog from the trait catalog already in memory
(`refresh_trait_catalog`, not the full `refresh_catalog`, which reads other classes'
modules from disk). Persistence goes through the background `plugin_save`. Before
this, the UI thread ran `save_to_path` (snapshot, serialize and write the whole
Blueprint) and then the trait index rebuild (scan the manifest and read
every trait and Blueprint file) on each click. The catalog now loads from the
persisted index, and trait file events/Refresh update the index on a background
thread. Compile (codegen + `syn`) never runs on a trait change; it runs on Compile/Play.

## What runs when a level plays

### Role entities

`ScriptDriver::start_roles` (`pulsar_game/src/scripting/driver/roles.rs`), in the
first script phase, spawns every usable role of the registry, in its order, as an
**entity of its class**, through the same world-command path scripts use for
`world::spawn` (`apply_spawn`). A role therefore has what any bound object has: an
entity, a `ClassInstance`, and `Self`-scoped subscriptions on its own entity
channel. A `Now` role (Game Mode, Game State, Player Controller) is spawned and its
script started within the phase (so they `begin_play` in that order, before the
level's own objects); an `EndOfFrame` role (the Player Character) is queued like a
script's spawn and applied at the end of the frame.

### Role lifecycle on the hub

The driver publishes the role lifecycle as built-in Gamma events
(`pulsar_events::builtin`, global channel), so a plugin's role, or any plugin,
reacts to play wherever the engine's roles do:

| Event | When |
|---|---|
| `RoleSpawned { role, entity }` | each role entity the session spawns (any registered role) |
| `GameBegan { level }` | after the roles spawned and the engine's start hooks ran |
| `PossessionChanged { controller, pawn, possessed }` | the local player's controller takes (`true`) or releases (`false`) its pawn |
| `GameEnded { level }` | at shutdown, after possession ended and `GameMode.on_game_end` ran, before the role entities go |

Events say what happened; trait methods are called directly (below), because they
return values and some run at a set point in the frame (`on_input` every frame
while possessed), which the hub's deferred delivery cannot give.

Role entities are **session objects, never level data**: no level-format field, no
editor outliner or save-path filtering. (Under Play-in-Editor they are visible in
the shared world while the game runs, like any `world::spawn` object.)

- **Ownership is tracked, not inferred.** The driver keeps the list of entities it
  spawned (`ScriptDriver::role_entities()`, `role_entity(role)`) and
  `end_play_all` (`TickLoop::end_scripts`, run by `PieSession::shutdown`, the
  standalone exit and `run_blocking`) despawns exactly those after their scripts
  ended. `embed.rs` adopts the editor's shared world store, so this is what keeps
  them from leaking back into the editor.
- **Hot reload re-binds.** Each role entity is also stamped with
  `pulsar_game::roles::RoleEntity { role }`. A session that finds stamped
  entities in the world it adopts (the previous session was dropped without a
  clean shutdown) re-binds the one whose class still matches the level's
  assignment and removes the rest, instead of spawning a second set. This happens
  whenever roles start, not only for `begin_script_reload` sessions: nothing but a
  previous session can have put a role entity in a world.

### Hooks and the trait query

Hook order: `GameMode.on_game_start`, `GameState.on_init`,
`GameMode.on_player_joined(0)`, `GameState.on_player_added(0)`; the next frame, once
the character runs: `PlayerCharacter.on_spawned`, `PlayerController.on_possess(pawn)`,
`PlayerCharacter.on_possessed(controller)`, then the input is routed (below). From the
frame after, while the pawn is possessed, `PlayerController.on_input(delta_seconds)`
every frame. When possession ends: `PlayerController.on_unpossess()`, then
`PlayerCharacter.on_unpossessed()` (not when the pawn is gone), then the input leaves
the pawn. At shutdown (`end_play_all`, before the scripts end): possession ends, then
`GameMode.on_game_end()`. Unset roles log `<Role> is not set for
this level; skipped`; no roles logs once; unusable ones (`NotInIndex`,
`MissingTrait`, no compiled script) warn and fail soft.

Hooks go **through the trait**, to **one dispatcher**: `RuntimeTraits::dispatch`
(`scripting/traits.rs`; `ScriptDriver::call_trait(entity, trait, method, args)`
builds the target):

1. resolves the trait to its id (`TraitDb::resolve`) and the method in the trait's
   `required_methods(id)` (supertraits included), so an inherited method or a renamed
   trait resolves;
2. calls every implementation on the entity: its **script class**'s handler for
   the event `<Class>.<trait method name>`, and each attached **native component**
   implementing the trait, through the method bound to it
   (`pulsar_traits::sources::bound_method`, the world registry's
   `invoke_component_method`, arguments converted by the script VM's type
   bindings). Unreal's `Execute_<Method>` reaches C++ and Blueprint
   implementations the same way;
3. checks the class's **contract** with the conformance code
   (`pulsar_traits::conformance::check`) over what the compiled module actually handles. A
   required method (no default body) with no handler is a **violation**: the
   driver reports it once per session, naming the class, trait and signature, in
   `DriverReport::failures` and a warning, and the hook returns
   `HookOutcome::Violation`. A method with a default body and no node placed is
   `HookOutcome::NotHandled`: no reaction, debug log only. A class that does not
   implement the trait at all is also a violation. (All built-in role methods are
   required, so for the role traits an unplaced node is a violation; the editor's
   compile step normally refuses such a Blueprint, so this means a stale module.)

The **entity-level query** is `ScriptDriver::entity_implements(world, entity, trait)`
(and `class_implements(class, trait)`; `entity_trait_check` keeps "cannot tell"
apart). The entity's class is its `ClassInstance` resolved through the class
registry; the class's declared trait ids are its implementer entry's in the index
(`Binding::ScriptClass` with the class's project-relative folder), widened by
`TraitDb::with_ancestors`; the index is the stored one when the project has it,
else `TraitIndex::live` (the engine's and the enabled plugins' traits and native
implementers). No file path or name string of the caller takes part, and the
runtime reads no Blueprint. A class the index does not list (content shipped
without it) is `TraitCheck::Unknown`, which hooks and possession treat as "trust
the level's role binding" rather than as a failure. Possession validates the pawn with
this query and skips (reports) when it is a definite no.

Behaviour notes: `on_possessed` now receives the controller's entity bits (the
trait's `controller_id`; it was the player id before the controller had an entity),
`on_possess` the pawn's. Covered by `scripting::tests::roles`.

### Input and possession

Possession is also what routes the local player's keys, mouse and pad to their
character. The game runtime names no plugin: it runs every enabled
`engine_backend::scene::game_hooks::GameSessionRegistration`, and the input plugin's
root submits one (`input_plugin::runtime::session`; the input runtime and its
`PlayerInputComponent` are `plugins/vendor/input_plugin`'s):

1. At the end of `advance_possession`, `possession_hooks` calls each hook's `possess`
   with the pawn and a `PossessionContext` (the project root, the level's `WorldRoles`).
   The input plugin attaches a `PlayerInputComponent` carrying
   the level's mapping (its World Settings field, `runtime::session::PLAYER_INPUT_MAPPING_KEY`, read through `WorldRoles::setting`) to the **pawn** entity (`attach_component`, by class
   name) and returns the instance; the driver stamps it so a later session over the
   same world can replace a leftover instead of doubling it. A level with no mapping
   possesses without input (a warning, not a failure); a mapping the runtime cannot
   attach is a failure, reported once. A game whose project disables the input plugin
   has no hook and no input.
2. The input plugin's per-frame `tick` is a native component tick like any other
   (`tick_live_components`, phase 1b of `TickLoop::run_tick`, before the script phase):
   it reads the world's `PlayerInputComponent`s, keeps its mappings in step, reads the
   raw keys/mouse/wheel the hosts publish on the hub (`windowed_app.rs`, `embed.rs`)
   and the pad, and publishes `input::<Action>.started | ongoing | triggered |
   completed | canceled` on the **pawn's** entity channel. A `Self`-scoped handler on
   the pawn hears its own player and nobody else. The events are delivered at the
   flush after the systems, so the handler runs in the same frame's script phase.
3. The project's action events (`input::Jump.triggered`, ...) are declared on the hub
   when the driver attaches to it (each hook's `begin`; the input plugin's
   `session::begin`), before any class links; a mapping only registers its own actions
   once it is active, which is after the pawn's script has subscribed.
4. `end_possession` (possession ends: the pawn is destroyed, the session ends, and
   later a "possess another pawn") calls `on_unpossess` / `on_unpossessed`, detaches
   what the hooks attached and clears the possession gate; the session's end runs each
   hook's `end` (the input plugin releases its devices and listeners).

A game links `input_plugin` (when its project enables it) **without its `editor`
feature**: its `keys`, `model`, `library`, `runtime` and `preview` modules only. The runtime finds the
project's mappings from `TickLoop`'s project root, set per world
(`input_plugin::runtime::set_world_project_root`), so Play-in-Editor and standalone
agree and two sessions in one process do not share a root.

**Free camera arbitration.** `possession::PossessionGate` is one shared flag owned by
the `TickLoop` (`possession_gate()`), set by the driver while a pawn is possessed. The
hosts' built-in `FreeCam` (windowed standalone, `windowed_app.rs`; Play-in-Editor,
`embed.rs`) is suspended from it each frame: `FreeCam::set_suspended` makes it ignore
keys and mouse motion and stop moving. With no possessed pawn it behaves as before. The
cursor click-to-capture / Escape-to-release logic in `windowed_app.rs` is unchanged on
purpose: raw mouse motion is only published to the hub while the cursor is captured, so
a possessed game needs that click to get mouse look.

**Blueprint side.** The Blueprint editor offers the project's input actions as event
entry points: `Events/Input/<Action>/On input::<Action>.<phase>`
(`features/events/engine_events.rs`, `input_events`, read from the project's
`.inputaction` files). The node's scope defaults to `Self` (the event's first field is
`entity`), which is what a character wants. The class compiles against the same event
(name and id) the runtime registers.

### Seams for what is missing

**Still missing for a playable game:** no control link between controller and character
beyond the events above (the controller cannot yet issue movement); no camera follows
the character (the view target is a separate step; the free camera only stands down);
the character spawns at the origin (no player start); one local player only;
`on_player_removed` is never called (no leave path yet); actor references are plain
integers; a shipped game finds the mapping in loose files only (the input library does
not read the content pak).

Where each plugs in:

- **Camera / view target**: set it from `advance_possession`, with `pending.character`
  (`possessed.pawn` after); `ScriptDriver::possessed_pawn()` already names it for the
  hosts, and `camera_selection::select_world_camera` is where a `CameraComponent` on
  the pawn would be picked up. A free camera that reads the hub instead of winit can
  replace the `FreeCam::set_suspended` gate (the gate then says when the hub's input
  belongs to the pawn).
- **Player start / movement**: the character's `WorldCommand::Spawn` in
  `spawn_role` carries `position: [0.0; 3]`; take it from a player start there. Movement
  is the pawn's own handlers on the `input::` events (or the controller's `on_input`).
- **Possess another pawn / leave**: `end_possession` is the exit; a new `possess(pawn)`
  would share `advance_possession`'s tail (hooks, `route_input`, the gate).

## Known limits

- `ui_types_common::codegen::generate_trait` ignores `supertraits` (resolving them needs names).
- The Trait Editor edits supertraits as text (one id/path per line), no picker.
- The runtime reads the stored index (`.pulsar/trait_index.json`) once per driver, so a
  trait edited while the game runs is seen after the next class reload (which clears the
  cache) or the next session. A packaged game ships no index today, so its classes are
  `Unknown` (the role binding is trusted) until the packager ships it.
- Stale unresolved references only re-resolve incrementally when a trait changes;
  a full rebuild re-reads every implementer's declared references.
- A role spawns a script class, so only implementers bound to one (`Binding::ScriptClass`)
  are offered for a role; a native component implements traits on the entities it is
  attached to.
- Input: the Blueprint editor reads the project's `.inputaction` files for its event
  nodes (its pin of the engine is by revision, so it does not link `input_plugin`) and
  mirrors `to_identifier` for an action's event name; a rename rule change there must be
  made in both. Actions created while Play-in-Editor runs have no declared events until
  the next session. The input library reads loose project files, not the content pak.
