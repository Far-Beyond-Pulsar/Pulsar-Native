# Traits

A **trait** is a named contract: a set of methods an asset promises to
provide. Traits are language-agnostic project data (`*.trait.json`). Blueprints
are the first implementers, and any plugin can add its own asset kind. The
engine uses traits to give gameplay *roles* to Blueprints (game mode, player
controller, ...), and the project index answers "who implements this?".

## Pieces

| Piece | Where | What |
|---|---|---|
| Trait asset | `ui_types_common::TraitAsset` (`types.rs`); editor in `plugins/trait_editor` | `.trait.json`: `id`, `name`, `methods`, `supertraits`, `meta` |
| Records | `engine_fs::trait_records` (`TraitRecord`) | Tolerant parse of a trait asset, no `ui_types_common` needed (runtime-safe) |
| Built-ins | `engine_fs::builtin_traits` | Engine-owned traits: `IntoGraph` and the four role traits |
| `TraitDb` | `engine_fs::trait_db` | Resolved in-memory model: maps, supertrait closure, effective implementers |
| Index | `engine_fs::BlueprintTraitIndex` (`blueprint_trait_index.rs`) | Scans/persists `.pulsar/blueprint_trait_index.json` (schema 2), incremental `update_path` |
| Conformance | `engine_fs::trait_conformance` | Does an asset provide the methods of the traits it declares? |
| Roles | `engine_fs::world_roles` | `ROLES` table, `RoleSelection` (level -> paths), `WorldRoles` (resolved + checked) |
| Reflection | `pulsar_reflection::trait_registry` (`TRAIT_REGISTRY`) | Published, language-agnostic snapshot per project |
| Plugin API | `plugin_editor_api::ProjectTraits` | `traits()`, `implementers(trait)`, `traits_of(asset)`, `implements`, `to_json` |
| Blueprint editor | `plugins/vendor/blueprint_editor/src/features/traits/` | Trait panel, Add-menu trait events, conformance at compile/preflight |
| Runtime | `pulsar_game` (`roles.rs`, `scripting/driver.rs`, `scripting/traits.rs`) | Spawns the selected role classes as session entities, answers "does this entity's class implement trait X", calls their trait events through the trait |

## Identity: ids, not paths

A trait's identity is its **id**; the path is where it currently lives.

- The id is embedded in the asset (`"id"`). The trait editor keeps it and mints
  one for a new trait. `engine_fs` assigns one to any trait asset that has none
  when the index is rebuilt (the file is rewritten once). If the file cannot be
  written, the id comes from the repo's durable asset-id manifest
  (`.pulsar/asset_ids.json`, kind `trait`, via `ProjectFileIndex::ensure_typed_asset_id`,
  which also follows file moves), and as a last resort it is `path:<path>`
  (not rename-safe; logged). Ids are also mirrored into the file index
  (`record_typed_asset_id("trait", ...)`) so pickers can look traits up by kind.
- A copied trait file carries the same id; the copy that sorts second gets a
  fresh id. A copy of a built-in becomes an ordinary trait.
- Blueprints store `blueprint_metadata.implemented_trait_ids` (authoritative)
  and `implemented_traits` (paths: the pre-id format, a fallback, and a hint
  refreshed on save). Resolution: ids first; then paths. A path beside ids that
  all resolve is only a stale hint after a rename and is ignored. A Blueprint
  with paths but no ids is **path only** (`BlueprintTraitEntry::path_only`): it
  still works, and the Blueprint editor adds ids when it loads the traits and
  writes them with the next save. Nothing rewrites a Blueprint behind your back.
- Renaming or moving a trait therefore keeps its implementers. References
  that name nothing are kept as `unresolved`, shown as "Missing trait", and
  never dropped silently.
- Supertrait references (`supertraits`: ids or paths) resolve the same way.

## The index

`BlueprintTraitIndex` is derived data. `rebuild` scans the project (and is what
opening a project does: providers have no reliable freshness contract).
`update_path(root, path)` re-reads **one** Blueprint or trait asset and updates
the persisted index; the Blueprint editor's save hook uses it. A trait change
only recomputes the derived paths and the references that newly (un)resolve;
no Blueprint file is read. Persisted output is sorted and deterministic.

Lookups are map-based (`TraitDb`): `blueprints_for_trait(id | path | name)`,
`traits_for_blueprint`, `resolve_trait`. **Supertraits are modelled**: a
Blueprint that declares `MyMode: GameMode` is an implementer of `GameMode`
(`Implementation::via` names the declared trait). Cycles are detected and do
not hang. The type system's `type-index/index.json` is cross-referenced
(`TraitRecord::type_index_id`).

Blueprints as the editor writes them have `//` header comments and
`format_version` 2; the index reads both (before this work it only accepted
header-less version 1, so editor-saved Blueprints were never indexed).

## Built-in role traits

Engine-owned. Defined once in `builtin_traits.rs` with fixed ids
(`pulsar.GameMode`, ...) and materialized into every project as ordinary
`types/traits/<snake>.trait.json`. At every rebuild the project copy is
**re-synced**: `id`, `meta.builtin`, `meta.contractVersion` and the methods are
the engine's; a project may edit `description` only. A copy whose methods drifted
is rewritten (warning). A hand-authored file at a built-in's path is adopted.
If the project cannot be written, `TraitDb` serves the engine's definition
anyway. To change a contract, edit the definition and bump `contract_version`.

| Role | Trait | Methods |
|---|---|---|
| Game Mode | `GameMode` | `on_game_start()`, `on_player_joined(player_id: u32)`, `on_game_end()` |
| Game State | `GameState` | `on_init()`, `on_player_added(player_id: u32)`, `on_player_removed(player_id: u32)` |
| Player Controller | `PlayerController` | `on_possess(pawn_id: u64)`, `on_unpossess()`, `on_input(delta_seconds: f32)` |
| Player Character | `PlayerCharacter` | `on_spawned()`, `on_possessed(controller_id: u64)`, `on_unpossessed()` |

Ids are plain integers until the spawn path hands Blueprints typed actor
references (a parameter type change is a contract version bump).

## Roles in a level

`engine_fs::world_roles::ROLES` is the one table: role -> trait id -> setting key
(`game_mode_blueprint`, `player_controller_blueprint`, `player_character_blueprint`,
`game_state_blueprint`). World Settings builds one picker per row, listing the
Blueprints that implement the role's trait (effective, so sub-traits count). The
fields on `WorldSettingsData` default to `None`, so older levels load. Adding a role:
a row in `ROLES`, a field + two match arms in `WorldSettingsData`, a built-in trait.

`WorldRoles::for_level(level_json, project_root)` resolves a level's selections
against the persisted index (read-only; `Unchecked` when there is no index, as in
shipped content): `Valid`, `NotInIndex`, `MissingTrait`, `Unchecked`.
`TickLoop::load_world_roles` is called when a level loads, in standalone
(`standalone::load_level`) and Play-in-Editor (`embed.rs`, from `target/pie/play.level`).

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

`trait_conformance::check_blueprint(db, blueprint_json)` checks every declared
trait. A method is provided by a custom event (`local_events`), a macro
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
(`BlueprintTraitIndex::publish`). Snapshots are per project (`trait_scope_key`).
A plugin adding its own implementer kind should extend the index (today only
Blueprints are scanned; `TraitImplementer::kind` is `blueprint`).

## Editing a Blueprint's traits (no hitches)

Adding/removing a trait is a metadata edit. The Add-menu entries are spliced into
the profile catalog from the trait catalog already in memory
(`refresh_trait_catalog`, not the full `refresh_catalog`, which reads other classes'
modules from disk). Persistence goes through the background `plugin_save`. Before
this, the UI thread ran `save_to_path` (snapshot, serialize and write the whole
Blueprint) and then `BlueprintTraitIndex::rebuild` (scan the manifest and read
every trait and Blueprint file) on each click. The catalog now loads from the
persisted index, and trait file events/Refresh update the index on a background
thread. Compile (codegen + `syn`) never runs on a trait change; it runs on Compile/Play.

## What runs when a level plays

### Role entities

`ScriptDriver::start_roles` (`pulsar_game/src/scripting/driver.rs`), in the first
script phase, spawns every usable role as an **entity of its class**, through the
same world-command path scripts use for `world::spawn` (`apply_spawn`). A role
therefore has what any bound object has: an entity, a `ClassInstance`, and
`Self`-scoped subscriptions on its own entity channel. Order: Game Mode, Game
State and Player Controller are spawned and their scripts started within the phase
(so they `begin_play` in that order, before the level's own objects); the Player
Character's spawn is queued like a script's and applied at the end of the frame.

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

Hook order (unchanged): `GameMode.on_game_start`, `GameState.on_init`,
`GameMode.on_player_joined(0)`, `GameState.on_player_added(0)`; the next frame, once
the character runs: `PlayerCharacter.on_spawned`, `PlayerController.on_possess(pawn)`,
`PlayerCharacter.on_possessed(controller)`. Unset roles log `<Role> is not set for
this level; skipped`; no roles logs once; unusable ones (`NotInIndex`,
`MissingTrait`, no compiled script) warn and fail soft.

Hooks go **through the trait** (`scripting/traits.rs`). `call_trait_hook(entity,
trait, method, args)`:

1. resolves the trait to its id (`TraitDb::resolve`) and the method in the trait's
   `required_methods(id)` (supertraits included), so an inherited method or a renamed
   trait resolves; the handler is the class's event `<Class>.<trait method name>`;
2. checks the class's **contract** with the existing conformance code
   (`trait_conformance::check`) over what the compiled module actually handles. A
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
registry; the class's declared trait ids come from its Blueprint
(`<class dir>/graph_save.json`, `engine_fs::declared_traits`), widened by
`TraitDb::with_ancestors`; the database is the persisted index when the project has
one, else the engine's built-ins (`TraitDb::builtin`). No file path or name string
of the caller takes part. A class whose Blueprint cannot be read (content shipped
without it) is `TraitCheck::Unknown`, which hooks and possession treat as "trust the
level's role binding" rather than as a failure. Possession validates the pawn with
this query and skips (reports) when it is a definite no.

Behaviour notes: `on_possessed` now receives the controller's entity bits (the
trait's `controller_id`; it was the player id before the controller had an entity),
`on_possess` the pawn's. Covered by `scripting::tests::roles`.

### Seams for what is missing

**Still missing for a playable game:** possession is only the two events (no
control link between controller and character); no input routing to the controller
(`on_input` is never called); no camera follows the character; the character spawns
at the origin (no player start); one local player only; `on_game_end`, `on_unpossess`,
`on_unpossessed`, `on_player_removed` are never called (no shutdown/leave path yet);
actor references are plain integers.

Where each plugs in, all in `advance_possession` / `start_roles`:

- **Input routing**: attach the controller's input component to the pawn at the
  end of `advance_possession` (both entities exist there), and call
  `call_trait_hook(controller, PlayerController, "on_input", ...)` from
  `run_role_hooks` each frame; the controller already has an entity channel.
- **Camera / view target**: set it from the same place, with `pending.character`.
- **Player start / movement**: the character's `WorldCommand::Spawn` in
  `spawn_role` carries `position: [0.0; 3]`; take it from a player start there.
- **Shutdown events** (`on_game_end`, `on_unpossess`): call them in `end_play_all` before `runtime.end_play_all`, while the
  scripts live.

## Known limits

- `ui_types_common::codegen::generate_trait` ignores `supertraits` (resolving them needs names).
- The Trait Editor edits supertraits as text (one id/path per line), no picker.
- The conformance check does not see a trait implemented by a plugin-defined asset kind.
- The runtime trait query reads the persisted index (`.pulsar/blueprint_trait_index.json`) once per
  driver, so a trait edited while the game runs is seen after the next class reload (which clears
  the cache) or the next session; without an index only the built-in traits resolve.
- Stale unresolved references only re-resolve incrementally when a trait changes;
  a full rebuild re-reads the Blueprints' declared references.
- Submodules changed: `crates/external/pulsar-reflection` (Pulsar-Reflection),
  `crates/third-party/graphy` (Graphy), `plugins/vendor/blueprint_editor`
  (Plugin_Blueprints). Merge dependency-first (see `CRATES.md`, "Submodule pins").
