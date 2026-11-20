Status: as built (as of 2026-11-20) - P1-P6 built and committed (970dfa1e5); its own 'uncommitted' note is stale

# Component event sources plan

Status: **implemented, uncommitted** (2026-11-19). Sections 1-11 are the design as written;
section 12 records P0; section 13 is what was built, where it differs from the design, and
what is not done. Read 13 first.

Goal: **the events a component offers to Blueprints follow the assets and values its
properties name, through one generic mechanism any plugin can use**, with typed payload
pins, no class or property names in the editor, and no I/O on any hot path. The first
client after animation is input: `On Input Action`, driven by the `PlayerInputComponent`'s
mapping.

## 1. Why

Components raise events scripts can hear, but three things are wrong today.

1. **Asset-declared events are hard-wired to animation.** The Blueprint editor knows
   `AnimationComponent`, `SkeletalMeshComponent` and the property keys `animation_graph` /
   `animation` (`blueprint_editor/src/features/events/engine_events.rs`:
   `is_animator_class`, `referenced_animation_asset_ids`, `animation_asset_ids_from_prefab`),
   and the compiler special-cases `animseq::`/`montage::`/`animgraph::` names
   (`blueprint_editor/compiler/src/lib.rs`, `component_event_name`). A second plugin cannot
   do the same without editing the editor. That breaks the layer rule (core and editor know
   no kind) the moment a second client appears.
2. **Input has no component events at all.** `PlayerInputComponent` declares none. Input
   events reach Blueprints through a separate path: `read_input_events` re-scans the
   project for `.inputaction` files, re-implements the action naming rule
   (`input_action_name`, a hand copy of `input_plugin::model::action::to_identifier`) and
   emits five flat `On input::<Action>.<phase>` palette entries per action. Payloads are
   seven untyped `f64`/`u64` fields, so a 2D action shows as `x`, `y`, `z` floats.
3. **Nothing is cached or incremental.** The animation catalog opens the file index and
   reads asset files on every palette build and every compile
   (`skm_editor/component/src/notifies.rs`, `component_animation_notify_events`); the input
   scan has a 3 second TTL cache keyed by project root only.

## 2. As built (what the design rests on)

| Fact | Where |
|---|---|
| `ComponentEventsRegistration { origin, class_name, events: fn(root, &HashSet<String>) -> ComponentEventCatalog }`, found by class name | `engine_backend/src/scene/component_events.rs` |
| `skm_editor` registers one for `AnimationComponent` | `skm_editor/src/runtime.rs:38` |
| Typed asset reference: `AssetRef<K: AssetKind>`, a UUID string on disk; `AssetKind` has `KIND: &str` and `EXTENSIONS` | `pulsar-reflection/.../asset_refs.rs` |
| `AnimationComponent.animation_graph: Option<AssetRef<AnimationGraphAssetKind>>` | `skm_editor/component/src/component.rs:231` |
| Asset id to path by kind: `ProjectFileIndex::path_for_asset_id_kind(kind, id)`; reverse `asset_ids_for_path`; dependency edges `replace_asset_deps`, `asset_used_by` | `engine_fs/src/file_index.rs` |
| File change feed: `engine_fs::events::subscribe()` (`Created`/`Modified`/`Deleted`) | `engine_fs/src/events.rs` |
| Registries iterate `pulsar_plugins::registered::<T>()`, skipping disabled plugins; `Contribution` gives category, origin, id | `pulsar_plugins/src/lib.rs` |
| Palette entry shape: `EventNode { node_type, inputs, outputs, properties }`, pins named after signature fields; an "On" node has one exec output `Body` and a `scope` property (`self`/`global`/`class`) | `blueprint_editor/compiler/src/palette.rs`, `compiler/src/lib.rs:166-190, 639-660` |
| The properties panel already renders a node's `properties` map | `blueprint_editor/src/ui_components/properties.rs:546` |
| Script types include `Object(String)` for registered value types | `pulsar_script_vm/src/types.rs` |
| Input runtime already publishes per action on the owner's entity channel, gated by `subscriber_count` so unheard events cost nothing | `input_plugin/src/runtime/events.rs:394` |
| `PlayerInputComponent.mapping` is a project-relative path `String`, not a typed reference | `input_plugin/src/runtime/component.rs` |
| Record migrations exist for prefab/component data | `skm_editor/src/runtime.rs` (`RecordMigrationRegistration`) |

## 3. Target design

Two layers. Layer A covers nearly everything and costs a component author nothing. Layer B
is an escape hatch for events that depend on plain values, not assets.

### 3.1 Layer A: the asset kind declares its events

A property typed `AssetRef<K>` already says what it points at. So the **asset kind**, not
the component class, owns the answer to "what events does an instance of this asset raise".

```rust
/// Registered once by the plugin that owns the asset kind.
pub struct AssetEventSource {
    pub origin: &'static str,
    /// `AssetKind::KIND` of the assets this source reads.
    pub kind: &'static str,
    pub families: fn() -> &'static [EventFamilySpec],   // static shape, see 3.3
    pub declare: fn(cx: &mut SourceCx, asset: &AssetId) -> Result<EventCatalog, SourceError>,
}
```

`SourceCx` is the only thing a source sees. It hands out the project root, the file index,
`read_asset(id)` (through `virtual_fs`, so remote and P2P projects work) and
`depend_on(kind, id)`. Calling `depend_on` records an edge in the cache (see 5); a source
never declares dependencies statically, so an animation graph that pulls in sequences and
montages just calls it as it walks.

The palette and the compiler ask one question of one function:

```rust
pub fn component_events(prefab: &PrefabView, root: &Path) -> Arc<EventSet>
```

It does, for each enabled component in the prefab:

1. `AssetSlots::of(class)` gives that class's reflected properties whose type is an asset
   reference, as `[(property name, kind)]`. Built once per class and cached for the plugin
   generation (3.2).
2. Read each such property from the prefab data by name and parse the id. No full
   deserialization.
3. Group by `(kind, id)`, sort (a `BTreeSet`), de-duplicate, and look up the cached
   `Arc<EventCatalog>` for each.
4. Return one `EventSet` per component instance: events carry the component they came from,
   so two animators on one class stay distinct (the existing component-reference
   subscription path).

No class name, no property name, no kind appears in the editor or the compiler. Deleting
`ComponentEventsRegistration` per class, `is_animator_class`, `referenced_animation_asset_ids`
and the `AnimationComponent` branch in `component_event_name` is part of the migration.

### 3.2 Reflection: say it on the property

The walk needs the property's asset kind. Reflection gets one defaulted method:

```rust
trait Reflectable {
    // existing ...
    /// `Some(K::KIND)` when this type is an `AssetRef<K>` or wraps one.
    fn asset_kind() -> Option<&'static str> { None }
}
```

`AssetRef<K>` returns `Some(K::KIND)`; `Option<T>` and `Vec<T>` forward. `PropertyMetadata`
gains `asset_kind: Option<&'static str>`, filled by the derive from `T::asset_kind()` (so
type aliases and wrappers work; the derive never matches on spelling). A defaulted method
is source compatible. Opt-out lives on the property: `#[property(events = false)]` for an
asset reference that raises nothing.

### 3.3 Families: the parametric event

A flat catalog does not scale for input: 100 actions times five phases is 500 palette
entries and 500 signatures compiled against, and the user wants one node. So a source
returns **families** as well as plain events.

```rust
pub struct EventFamilySpec {
    pub id: &'static str,                 // "input.action"
    pub label: &'static str,              // "On Input Action"
    pub params: &'static [ParamSpec],     // action (choice), phases (multi-choice), scope
}

pub struct EventCatalog {
    pub events: Vec<EventDescriptor>,          // plain events (animation notifies)
    pub families: Vec<FamilyInstance>,         // parametric (input actions)
    pub diagnostics: Vec<SourceDiagnostic>,
}

pub struct FamilyInstance {
    pub family: &'static str,
    pub choices: Vec<Choice>,                  // one per action: id, label, extra typed data
    pub resolve: fn(&Choice, &Params) -> Result<ResolvedEvent, ParamError>,
}

pub struct ResolvedEvent {
    pub signature: EventSignature,             // script-typed pins
    pub subscriptions: Vec<EventName>,         // concrete runtime event names
    pub lowering: Vec<FieldLowering>,          // how pins are built from payload fields
}
```

Properties:

- **Palette cost is O(families), not O(choices x phases).** One "On Input Action" entry
  appears per family per component, however many actions the mapping binds.
- **Compile cost is O(nodes used).** `resolve` runs only for nodes in the graph, not for
  every action in the mapping.
- **Parameters are node properties**, rendered by the existing properties panel. A
  `ParamSpec` of kind `Choice` shows a dropdown fed from the catalog's `choices`;
  `MultiChoice` shows checkboxes.
- **Typed payload.** `resolve` returns script `Type`s directly, so a 2D action yields a
  `Vector2` pin, not `x`/`y`/`z`. `FieldLowering::Pack { ty, from: [x, y] }` tells the
  compiler to build the value from the runtime event's plain `f64` fields in bytecode.
  The wire format stays as it is (no change to `HubBinding::publish`); typing lives in
  the signature and the lowering.
- **Stable identity.** Choices are keyed by the asset id (`AssetRef`), never by name. The
  runtime name (`input::Jump.triggered`) is resolved at compile time from the asset, so
  renaming an action's identifier recompiles correctly.

### 3.4 Layer B: the escape hatch

Events that depend on component values, not assets (a mode enum that changes the payload,
the voxel generator version), use a reflected method:

```rust
#[world_component]
impl MyComponent {
    #[bp_event_source]
    fn dynamic_events(&self) -> EventCatalog { ... }
}
```

It reads `&self` (the prefab data in the editor), must be pure, and goes through the same
`EventSet` merge and cache, keyed by a hash of the component's data. It is a P5 item and is
not needed for animation or input; building it later is cheap because the merge is shared.

## 4. Input as the second client

### 4.1 The component

`PlayerInputComponent.mapping` becomes `Option<AssetRef<InputMapAssetKind>>`, edited by the
existing `ui_common::asset_ref_editor` picker (extension-filtered to `.inputmap`, with
Clear). A record migration reads the old path string, resolves it through the file index
to an id, and writes the reference; an unresolvable path is kept in a `legacy_mapping`
field and reported as a diagnostic, not dropped. `InputMapAssetKind` and
`InputActionAssetKind` live next to the other kinds in `pulsar-reflection`.

### 4.2 The source

`input_plugin` registers one `AssetEventSource { kind: InputMapAssetKind::KIND, .. }`.
`declare` reads the `.inputmap`, follows each entry's action reference to the
`.inputaction` (through `SourceCx`, so edges are recorded), and returns one family
instance `input.action` whose choices are the actions the mapping binds, each carrying its
`ValueType`. It reuses `input_plugin::model` directly, so `input_action_name` and
`read_input_events` in the editor are **deleted**, not kept in sync.

A class with no `PlayerInputComponent` (the level's player controller brings the mapping at
possession) falls back to a project-wide action source: the same family over every
`.inputaction`, labelled as unscoped. A component with a mapping narrows it.

### 4.3 The node: `On Input Action`

Mockup shown in the design discussion; the contract is:

| Property | Kind | Notes |
|---|---|---|
| Action | `Choice` | the mapping's actions, stored as the action asset id |
| Fires on | `MultiChoice` | Started, Ongoing, Triggered, Completed, Canceled |
| Scope | `Choice` | Self (default), Global; the existing `scope` property |

Pins derive from the action's `ValueType`:

| ValueType | Value pin |
|---|---|
| Digital | `Pressed: bool` |
| Axis 1D | `Value: f32` |
| Axis 2D | `Value: Vector2` |
| Axis 3D | `Value: Vector3` |

Always present: `Elapsed`, `Triggered For` (seconds), `Entity`. Execution outputs follow
the phase model (decision D1 below). Validation (Blueprint problems list and the panel):
an action the mapping no longer binds, a deleted action or changed value type (the node
keeps its last-known pins and wires and shows an error, never silently drops them), no
phase selected, and Ongoing on a digital action (fires every frame while held).

D1, the phase model, **decided (2026-11-19): one exec output per selected phase**
(Unreal's Enhanced Input shape), sharing the data pins. The compiler's event node must
support several exec outputs, each compiling to its own subscription (`ResolvedEvent`
already lists `subscriptions`). Selecting or deselecting a phase adds or removes that exec
pin; wires on a deselected phase are kept as orphaned until the phase is re-enabled or the
wire is removed. Rejected: a single `Body` plus a `Phase` enum output and a Switch node.
P0 spikes the multi-exec event node in `compile.rs` first, since everything in P4 and P5
rests on it.

### 4.4 Runtime

No change to the hub: `HubBinding::publish` already raises the per-action events on the
owner's entity channel and skips events nobody hears. The compiler lowers a node to one
subscription per selected phase with the node's scope. `Self` scope hears only its own
player. Typed values are built in bytecode from the existing fields, so there is no per
event allocation added on the hot path.

## 5. Performance

Where cost lands, and the design for each. Targets are budgets to **measure in P0 and hold
in CI**, not claims about today's numbers.

| Path | Today | Design | Budget |
|---|---|---|---|
| Palette rebuild on prefab edit | opens index, reads files, per call | cached `Arc<EventCatalog>` per `(kind, id)`; one prefab walk by property name | cache hit under 1 ms |
| Compile | rebuilds all signatures | `resolve` for graph nodes only; cached `Arc` | under 1 ms added on a hit |
| Cold read of an `.inputmap` of 100 actions | (rescans project) | one mapping read plus its action files via the index | under 10 ms |
| Palette entries | 5 x actions flat | families: entries per component, not per action | O(1) in action count |
| Runtime event | allocates `Vec<DynValue>` when heard | unchanged; skipped when unheard; typed pack in bytecode | no new allocation |

Mechanics:

- **Cache key and invalidation.** `EventCache` maps `(kind, asset id)` to
  `Arc<EventCatalog>` with a content stamp (file size and modified time from the index).
  A single subscriber on `engine_fs::events::subscribe()` handles `Modified` and `Deleted`:
  it maps the path to asset ids (`asset_ids_for_path`) and evicts them **and everything that
  recorded a `depend_on` edge to them**, transitively, using the edges the source recorded.
  Nothing polls and nothing uses a TTL, so staleness is impossible rather than unlikely.
- **No work when nothing references an asset.** An empty slot (`None`) costs a property
  read. A class with no asset-reference properties costs one table lookup.
- **Class tables are built once.** `AssetSlots::of(class)` is computed lazily from
  reflection and invalidated only when `pulsar_plugins::generation()` changes.
- **Cheap merge.** Catalogs and signatures are `Arc`s; an `EventSet` is a vector of cloned
  `Arc`s, ordered deterministically, so the compile cache key (a hash over the signatures
  the graph uses) is stable between runs.
- **Off the UI thread.** A cache miss is an editor task (`.agents/EDITOR_TASKS.md`: every
  async job is a task with a span-scoped log). The palette shows the last-known set while a
  refresh runs, tagged with a generation counter so a stale result is dropped. A hit is
  synchronous and does no I/O.
- **Cook equals editor.** `component_events` is a pure function of prefab data and the
  project; the cook and the editor call the same code, so a script cannot compile in one and
  not the other.
- **Measurement.** A `criterion`-style bench (or the project's equivalent) over: 1 and 100
  action mappings, 50 component prefab, cold and warm; asserts the budgets above. A tracing
  span per phase (walk, lookup, declare, resolve) makes regressions attributable in the
  Tasks window.

## 6. Dependencies, renames and broken references

- A Blueprint that uses an event family records `replace_asset_deps` edges to the action
  and mapping assets. Renaming an action's identifier or editing its value type marks
  dependent classes stale through `asset_used_by`, so the module is recompiled rather than
  silently left on the old name.
- Deleting an asset leaves the node in an **orphaned** state: last-known signature kept
  (a snapshot of its pins in the graph JSON), wires kept, a compile error naming the
  missing asset. It never disappears.
- The source's `diagnostics` (an unreadable mapping, an action file that fails to parse)
  reach the Blueprint problems list with the asset path, not a log line.

## 7. Migration

1. Animation moves onto `AssetEventSource` first. It is the existing client with the most
   behaviour, so it proves the mechanism with tests that already exist (`phase3_tests`,
   `component_events` tests).
2. Delete `ComponentEventsRegistration`, `animation_catalog`, `is_animator_class`,
   `referenced_animation_asset_ids`, `animation_asset_ids_from_prefab`, and the
   `AnimationComponent` branch in `component_event_name`. Saved graphs keep loading: node
   type names are unchanged and the legacy spellings stay in the compiler's normaliser.
3. Input: component migration, source, family, node, then delete `read_input_events`,
   `input_action_name`, the 3 second cache and the flat `input_events` palette path.
   Saved `On input::Jump.triggered` nodes migrate on load to `On Input Action` with the
   matching action and phase; a name that matches no action stays as a plain event node
   with a warning.
4. `just arch` must pass: nothing in core or editor names a plugin or kind.

## 8. Phases

| Phase | Work | Done when |
|---|---|---|
| P0 | Answer the open facts (done); spike multi-exec (done); add the bench harness; measure today's palette and compile cost | numbers recorded in section 12 |
| P1 | `Reflectable::asset_kind`, `PropertyMetadata.asset_kind`, `#[property(events = false)]` | reflection tests; no consumer yet |
| P2 | `AssetEventSource`, `SourceCx`, `EventCache` with dependency eviction, `component_events` walker | cache tests: hit, evict on edit, evict through a dependency |
| P3 | Animation onto it; delete the class registration and editor special cases | existing animation event tests pass unchanged |
| P4 | Families, `ParamSpec` rendering in the properties panel, lowering, compiler support for family nodes (and D1) | a synthetic family compiles and runs |
| P5 | Input: typed `mapping`, migration, source, `On Input Action`, validation, orphaned state, delete the old path | end to end: change the mapping in the component panel, the node's action list changes, a typed `Vector2` reaches a handler |
| P6 | Layer B `#[bp_event_source]`; docs; benches in CI | one in-tree component uses it |

Per the project rule for multi-phase work: build every phase, log non-blocking issues in
"Issues" below, resolve them in a cleanup pass.

## 9. Tests

- Reflection: `asset_kind` through `Option`/`Vec`/alias; opt-out.
- Cache: hit returns the same `Arc`; modifying an asset evicts it; modifying a dependency
  evicts its dependents; deleting evicts; a plugin generation change drops the class table.
- Walker: two animators on one class stay distinct; empty slot; unknown id gives a
  diagnostic; deterministic order.
- Families: `resolve` types per `ValueType`; a deleted action leaves an orphaned node that
  keeps wires and fails compile with the asset named.
- Compile: a graph using `On Input Action` for Move produces one subscription per selected
  phase and a `Vector2` local; scope `Self` filters to the owner.
- Runtime (`plugins/installed/tests`): a player object with a mapping, a scripted class,
  press a key, the handler runs in the same frame (the existing events guarantee).
- Migration: old `mapping` string with a resolvable and an unresolvable path; old
  `On input::Jump.triggered` nodes.
- Bench: the budgets in section 5.

## 10. Comparison (from memory, not re-checked against current docs)

- **Unreal Enhanced Input** has an Input Action node with exec pins per trigger phase and
  `ActionValue`, `ElapsedSeconds`, `TriggeredSeconds` outputs; the node is not tied to a
  mapping context, so any action is offered. Ours keeps that shape and adds the
  component's mapping narrowing the list, a value type fixed by the action, and
  reference-by-id.
- **Unity Input System** `PlayerInput` can send messages by action *name string* or raise
  UnityEvents; names are strings and a rename breaks callers silently. Ours keys on the
  asset id and flags a break at compile time.
- **Godot** exposes input actions by name strings in `Input`. Same weakness.

## 11. Risks and open decisions

- **D1** phase model: decided, exec per phase (see 4.3).
- Compiler change for several exec outputs and for family nodes is the largest single risk;
  it should be spiked in P0 against `compile.rs` tests.
- `Reflectable` is in an external crate (`crates/external/pulsar-reflection`); P1 is a
  submodule change and merges dependency first (see "Submodule pins" in `CRATES.md`).
- Families add a concept. If only input ever uses them, the flat catalog plus a node
  property would be simpler; the case for families is any asset that declares many
  similar events (animation notifies per sequence already do), which P3 should confirm
  before P4 is built.
- The fallback project-wide action source can offer actions no mapping binds; the node
  shows this as a warning, not an error.

## 12. P0 findings (2026-11-19)

Status of P0: the seven open facts below are answered and the compiler spike passes. The
baseline measurements and the bench harness are **not done** (see "Still open in P0").

| # | Question | Answer | Effect on the design |
|---|---|---|---|
| 1 | Is `AssetRef<K>` reflectable? | Yes by consequence: `Option<T>: Reflectable` needs `T: Reflectable`, and `asset_refs.rs` already uses `<Option<AssetRef<K>> as Reflectable>::type_info`. The editor hint is keyed by the `TypeId` of `Option<AssetRef<K>>`, one per kind, and `PropertyMetadata` carries no kind string. I did not find the `Reflectable for AssetRef` impl itself (it is not in `asset_refs.rs`). | The `Reflectable::asset_kind()` method in 3.2 stands (it forwards through `Option` and `Vec`, and needs no per-kind registration). Alternative if the trait change is unwanted: the per-kind macro also submits a `(TypeId, KIND)` inventory item. Find the impl before P1. |
| 2 | Is prefab component data typed or raw JSON? | Raw `serde_json::Value`: `PrefabComponent { slot_id, class_name, enabled, data }` (`pulsar_class/src/prefab.rs`). | The walker reads a property by name from JSON, as planned. `slot_id` is the stable component-instance key, so two animators on one class stay distinct. |
| 3 | Does the VM have `Vec2`? | Yes: `pulsar_script_math` registers `Vec2` and `Vec3` as `Type::Object` value types. The pin label "Vector2" in the mockup is display only; the type is `Object("Vec2")`. | Typed 2D/3D pins work as designed. |
| 4 | Can the compiler's event node have several exec outputs? | Not as built (`follow_all_exec` runs every exec output of an event node in one handler). **Spike: yes, a small change.** An `EventFn` gains `only_exec: HashMap<node, pin>`; a node absent from it runs all outputs, as before. A new node `event::on_each` with an `events` property (`pin=event,...`) expands to one handler and one subscription per exec output, sharing the data pins; mismatched payloads are a compile error. Two new tests pass and the existing compile tests still pass. | D1 is feasible and contained. The production family node produces the same `(exec pin, event name)` list from `resolve`. |
| 5 | Is there a content stamp in the file index? | `files` has `size` and `modified` columns (`file_index.rs`). | Usable as the cache backstop. Fs events remain the primary invalidation: a same-size edit inside the timestamp resolution would be missed by the stamp alone. |
| 6 | Can a record migration resolve `mapping` path to an id? | **No.** `RecordMigrationRegistration` is a pure function over JSON entries with no project root or index. | Decision D2 below. |
| 7 | Cost of `path_for_asset_id_kind`? | A SQLite query behind a `Mutex`, one `prepare` per call, per id. | The cache keeps an id-to-path map (evicted on delete and move events), and the index gets one batched call for N ids, using `prepare_cached`. |

D2, migrating `PlayerInputComponent.mapping`: because migrations cannot see the index,
the field keeps accepting the old path string (`legacy_mapping`) and gains the typed
`mapping: Option<AssetRef<InputMapAssetKind>>`. A root-aware upgrade runs where the index
exists (on prefab and scene open in the editor, and at cook) and rewrites the reference;
until then the runtime resolves the legacy path lazily. An unresolvable path stays in
`legacy_mapping` with a diagnostic. Alternative considered: extend
`RecordMigrationRegistration` with a root-aware entry; larger and touches every plugin.
This replaces the "record migration" wording in 4.1.

### The spike

Changes, uncommitted, in the `blueprint_editor` submodule working tree
(`compiler/src/lib.rs`, `compiler/tests/compile.rs`):

- `EventFn.only_exec`, set by the new `declare_each_event`, read in `compile_events`.
- The spike duplicates the registration tail of `declare_events` inside
  `declare_each_event`; the production version unifies them.
- Tests: `one_event_node_with_an_exec_output_per_event_compiles_one_handler_each`
  (two subscriptions, two handlers, each runs only its own chain; an event with no exec
  pin is not subscribed) and `an_each_event_node_with_mismatched_payloads_is_an_error`.

### Still open in P0

- Baseline numbers for palette rebuild and compile (no measurements taken yet).
- The bench harness for the section 5 budgets.
- Find the `Reflectable for AssetRef<K>` impl (finding 1).
- Decide whether to keep the spike's `event::on_each` as the production node type name or
  rename it when families land in P4.

### Original open questions (answered above)

1. Whether `AssetRef<K>` and `Option<AssetRef<K>>` implement `Reflectable` today, and what
   `PropertyMetadata.type_info` shows for them, or whether the `asset_kind` method needs a
   different hook.
2. Whether the prefab component data the palette reads is the typed struct or raw JSON
   (animation reads JSON keys today); the walker assumes it can read a property by name.
3. Whether the script VM has a registered `Vector2` (the code shows `Object(String)` and
   mentions `Vec3`); input needs 2D.
4. Whether the compiler's event node can carry several exec outputs, and how much a family
   node costs it.
5. Whether the file index stores a usable content stamp, or the cache should use fs
   size and modified time directly.
6. Whether a record migration can resolve a path to an id at load, which needs the file
   index at that point.
7. `ProjectFileIndex::path_for_asset_id_kind` cost per call (it is a database query): the
   walker may need to batch.

## 13. As built (2026-11-19)

Everything is in working trees, **uncommitted**, across the main repo and three submodules
(see "Where it lives"). Tests are named below; nothing was run in the editor UI.

### What exists

| Phase | Built | Where |
|---|---|---|
| P1 reflection | `AssetRefKindHint` inventory and `asset_kind_of(&RuntimeTypeInfo)` (looks through `Option` and `Vec`); the per-kind reflection macros submit the hint, so a kind that can be a property is always listed; new kinds `InputMapAssetKind` (`inputmap`) and `InputActionAssetKind` | `pulsar-reflection` `asset_refs.rs`, `lib.rs` |
| P2 sources | `AssetEventSource` (+ `for_kind::<K>`), `SourceCx` (root, index, `resolve` id-or-path, recorded file reads, `watch_extension`), `EventCache` (lazy `engine_fs` change feed, exact-file and extension eviction, per project), `asset_slots` (reflection walk per class, kept until `pulsar_plugins::generation()` changes), `referenced_assets`, `component_events` | `engine_backend/src/scene/asset_event_sources.rs` |
| P3 animation | The animation graph kind registers a source; its grouping and text moved from the editor into the plugin; the editor's animation special cases are deleted; the compiler's rule is "an event name containing `::` is asset-owned" (any plugin, and graphs saved under the former class name still compile) | `skm_editor/src/event_source.rs`, `blueprint_editor` `events/asset_events.rs`, `compiler` `is_asset_owned_event` |
| P4 families | `EventFamily`, `FamilyParam`, `ResolvedFamily`; compiler node `event::family::<id>` (one handler and one subscription per exec output via `EventFn.only_exec`, data outputs built from the raw event's fields by a `lowering` property: `f64`, `entity`, `bool`, `vec2`, `vec3`); palette entry per family and class; profile `pin_commands` reshapes pins and rewrites the compile snapshot (`events`, `lowering`) when a parameter changes; the properties panel draws a family's parameters as chips; `update_family_param` is one undo step | `component_events.rs`, `compiler/src/lib.rs`, `canvas_profile.rs`, `ui_components/properties/shared_controls.rs`, `graph_canvas_panel.rs` |
| P5 input | The `inputmap` kind's source answers with one family `input.action`: parameters `action` (a choice of the mapping's bound actions, by action asset id) and `phases` (multi), value pin typed from the action (`Pressed: bool`, `Value: f64 / Vec2 / Vec3`), plus `Elapsed`, `TriggeredFor`, `Entity`; the component's `mapping` path is registered as an `AssetPathSlot` | `input_plugin/src/event_source.rs`, `registrations.rs` |
| P6 | Integration test with the real plugins; a loose cache-hit guard test | `plugins/installed/tests/component_event_sources.rs` |

Tests that cover it: `pulsar_reflection` `asset_kind_hint_tests`; `engine_backend`
`asset_event_sources::tests` (8); `blueprint_compiler` `engine_events::*each_event*` (2) and
`family_nodes::*` (2); `blueprint_editor_plugin` `events::asset_events::tests` (9, run with
`--features builtin`); `skeletal_animation_plugin` `event_source::*` (3); `input_plugin`
`event_source::tests` (6); `installed_plugins` `component_event_sources` (5). `cargo check
--workspace` is clean. The tests that already existed for the touched crates pass.

### Where the build differs from the design

1. **Path slots, not a typed `mapping` property.** Design 4.1 made `PlayerInputComponent.mapping`
   an `Option<AssetRef<InputMapAssetKind>>`. That would have changed what the input runtime
   resolves each frame (a typed reference's id is the project manifest's, not the mapping file's
   own id) and needed an upgrade of saved scenes that migrations cannot do (finding 6). The
   property stays a path. `AssetPathSlot` is the bridge: a plugin says "this `String` property
   names a mapping". New components should use `AssetRef<K>`, which needs no registration.
   `InputMapAssetKind` and its reflected reference exist for that.
2. **The flat per-phase input palette stays.** `read_input_events` and `input_events` are not
   deleted. A class with no player input component has no mapping to narrow by, and still gets
   the project-wide actions (decision 13). A class that has one now sees both the family node
   and the flat entries.
3. **Families carry `events`, not per-choice cost.** The compiler's known-event list still gets
   every event of every family (5 per action); it is a flat conversion, not a per-frame cost,
   but it is O(actions) per compile. Resolving only the used nodes (design 3.3) is not done.
4. **The properties panel uses chips, not dropdowns.** A mapping with many actions will wrap
   to many rows. A searchable picker would be the next step.
5. **No `Reflectable::asset_kind()` method.** The hint registry is what fits how reflection
   registers asset references today (finding 1's alternative); it needs no trait change.
6. **Cache backstop.** Invalidation is by the change feed only; there is no size/modified
   stamp check on a hit (a hit does no I/O by design). A change made while no consumer
   called the cache is seen on its next call, not lost, because the feed is drained lazily.

### Macros (added after the first pass)

- **`#[property(events = false)]`** (`engine_class_derive`): the derive submits a
  `pulsar_reflection::PropertyEventsOptOut` for the field (an inventory entry, so
  `PropertyMetadata` and its construction sites are unchanged); `asset_slots` skips it
  (`property_declares_events`). Test: `opt_out::a_property_can_opt_out_of_raising_events`.
- **`#[bp_event_source]`** (Layer B): on one `&self` method of a `#[world_component]` impl
  returning `Vec<pulsar_world_registry::DynamicEvent>`; the macro submits a
  `ComponentEventSourceRegistration` that decodes the component's saved data and calls it. The
  walker merges the answers with the asset events, cached per distinct saved data
  (`EventCache::dynamic_catalog`, 512 entries). The descriptor must be namespaced
  (`EventDescriptor::dynamic_in`) so the compiler treats it as asset-owned; the component raises
  it itself by that name. Tests: `value_dependent::*`.

### Not built

- **Baseline measurements and a benchmark harness.** Only the loose guard in
  `a_cache_hit_is_cheap`. No numbers for palette rebuild or compile were taken, so the
  section 5 budgets are still targets.
- **The orphaned-node UI.** A family node keeps its pins, wires and snapshot when its action
  goes away (`family_commands` returns nothing for parameters that do not resolve), and the
  parameter panel shows why; the Blueprint problems list and the compile error do not yet
  name the missing action (the compile says the event is not declared).
- **Asset dependency edges** (`replace_asset_deps`): a Blueprint using a family is not marked
  stale when an action is renamed.
- **The `just arch` gate was not run** (no Python in this shell). Checked by hand: no kernel
  file names a kind word in an identifier or string; comments were reworded to be safe.
- **Not run in the editor:** the palette, the node, the chips and pin reshaping are covered by
  unit tests of their data and commands, not by looking at them.

### Where it lives (uncommitted)

- Main repo: `crates/core/engine_class_derive/src/{derive_engine_class.rs, properties.rs,
  world_component.rs}`, `crates/core/pulsar_world_registry/src/{lib.rs, component_events.rs}`,
  `crates/core/engine_backend/src/{lib.rs, scene/{mod.rs, component_events.rs,
  asset_event_sources.rs}}`, `plugins/installed/tests/component_event_sources.rs`, this plan,
  `AGENTS.md`, `PLUGIN_API_REFACTOR_PLAN.md`, submodule pointers.
- `crates/external/pulsar-reflection`: `asset_refs.rs`, `lib.rs`.
- `plugins/vendor/blueprint_editor`: `compiler/{src/lib.rs, tests/compile.rs, Cargo.toml}`,
  `src/{canvas_profile.rs, palette_catalog.rs}`, `src/features/{events/{asset_events.rs,
  engine_events.rs, mod.rs}, compilation/compiler.rs}`, `src/editor/graph_canvas_panel.rs`,
  `src/ui_components/{properties.rs, properties/shared_controls.rs}`.
- `plugins/vendor/skm_editor`: `src/{event_source.rs, runtime.rs, lib.rs}`, `Cargo.toml`.
- `plugins/vendor/input_plugin`: `src/{event_source.rs, registrations.rs, lib.rs}`.

Submodule changes merge dependency-first (`CRATES.md`, "Submodule pins"): reflection, then
the plugins, then the Pulsar pin bump.

## Issues

1. **`pulsar_reflection` `world_state` tests fail** (`process_wide_state_goes_through_the_runtime`,
   `unlisted_state_is_reported`), with and without these changes. Not investigated.
2. **`blueprint_editor_plugin` lib tests need `--features builtin`**; without it the crate's
   own tests do not compile (`traits/source.rs` tests use feature-gated functions).
3. **Legacy single-sequence animators.** The old palette also offered events for a component
   that named a single animation sequence under the `animation` key (the one-release
   compatibility field, not a reflected property). A reflection walk cannot see it, so those
   notifies are no longer offered through the palette for such components. The field's own
   doc says it goes when the window closes; if it must stay, give it an `AssetPathSlot`.
4. **`on_each` and the family node share one compile path**, and the spike's duplicated
   registration tail in `declare_each_event` should be folded into `declare_events`.
5. **`ProjectFileIndex::path_for_asset_id_kind` is one query per id** (design finding 7); the
   cache hides it on hits, a cold start of a large prefab does N queries. A batched call is
   not added.
6. **Palette duplicates for classes with a player input component** (build difference 2).
