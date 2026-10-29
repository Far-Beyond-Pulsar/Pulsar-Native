# Animation assets, references, compatibility and runtime (Phases 1–3)

How the animation family of assets is stored, linked and checked (Phase 1), how
they run (Phase 2: the compiled graph, per-component instance state, state
machines, Montages, IK, retargeting, Pose Search and the engine integration) and
how notifies, events, root motion and physics ownership behave (Phase 3, the last
section).
Plan: `docs/animation-system-implementation-plan.md`; audit:
`docs/animation-phase0-audit.md`.

## Decisions in force

| # | Decision |
|---|---|
| D1 | `.montage` is the only Montage extension. `anim_montage`/`anim_graph` are no longer accepted as extensions (they remain the `KIND` strings recorded in the file index). The mockup's `.animmontage` is to be corrected. |
| D2 | The **mesh** is the only Skeleton authority. `SkeletalMeshComponent` has no authored `skeleton`; the skeleton is read from the mesh header. An old saved `skeleton` is read once and, if it disagrees with the mesh, reported as a repairable diagnostic; the next save drops it. |
| D3 | All four new assets (`.animretarget`, `.ikrig`, `.ikretarget`, `.posedb`) are in scope. |
| D4 | The animation Graphy profile lives in `skm_editor` (not yet written; Phase 4/2). Its data model is `skeletal_assets::anim_graph::catalogue`. |
| D5 | Transitions are nodes (`sm.transition`), not graph-edge properties. |
| D6 | `ProjectFileIndex` has an `asset_deps` table. Per-kind extractors (`skeletal_assets::dependencies::extract`) feed it; "Used By" is a query over it. |
| D8 | Notify states are a separate `notify_states` list in `.animseq`. |
| D9 | The direct `animation` ref (and `looping`) on the component is **compatibility for one release**: it decodes into the internal single-sequence player (a compiled one-node graph inside the same `AnimInstance`, no `.animgraph` file) and is written back under the same keys. `animation_graph` is the authored source. There is one runtime, not two. |
| D11 | Small generated fixtures are committed (`skeletal_assets/tests/fixtures`). |

## Asset table

All files are JSON except `.skmesh` (binary: `PSKM` magic, version, JSON header,
vertex payload). Every format carries `version`, a stable `id` and keeps fields
it does not know (`extra`, flattened) so a newer writer's data survives a round
trip. Missing `id` is an error in current versions (never invented on read);
the v1 migrations assign one once and report it.

| File | Kind (`AssetKind::KIND`) | Rust type | Version | Owns | References |
|---|---|---|---|---|---|
| `.skeleton` | `skeleton` | `assets::SkeletonAsset` | 1 | bones, bind pose, sockets | `preview_mesh` (hint) |
| `.skmesh` | `skmesh` | `skmesh::SkeletalMeshAsset` | 1 | skin, LODs, material slots | **Skeleton** (authority) |
| `.animseq` | `animseq` | `assets::AnimSeqAsset` | 1–4 (written lazily) | tracks, curves, root motion, notifies, **notify states**, **payload schemas** | Skeleton; `preview_mesh` (hint) |
| `.blendspace` | `blendspace` | `blend_space::BlendSpaceAsset` | 2 | 1D/2D axes, samples, sync | Skeleton, Sequence per sample |
| `.montage` | `anim_montage` | `montage::AnimationMontageAsset` | 2 | slot tracks, segments, sections, blends, interrupt policy, notifies | Skeleton, Sequence per segment |
| `.animgraph` | `anim_graph` | `anim_graph::AnimationGraphAsset` | 2 | Graphy `GraphDescription`, parameters, sub-graphs | Skeleton; Sequence, Blend Space, Montage, IK Rig, Retarget, Pose DB, linked Graph per node property |
| `.animretarget` | `anim_retarget` | `retarget::RetargetProfileAsset` | 1 | bone/chain mapping, retarget poses, root policy | source/target Skeleton, optional source/target IK Rig |
| `.ikrig` | `ik_rig` | `ik_rig::IkRigAsset` | 1 | chains, goals, solver stack (2-bone, FABRIK), joint limits | Skeleton |
| `.ikretarget` | `ik_retarget` | `ik_retarget::IkRetargetAsset` | 1 | per-chain FK/IK settings, retarget poses, root settings | source/target IK Rig |
| `.posedb` | `pose_db` | `pose_db::PoseDbAsset` | 1 | sources, feature channels, sampling, search settings | Skeleton, Sequence / Blend Space sources |

Source of truth for the types is `plugins/vendor/skm_editor/skeletal_assets`
(submodule `skm_editor`); the typed markers are in `pulsar_reflection`
(`asset_refs.rs`). Skeleton/rig/profile references are `Option` so the create
menu can write a valid empty asset; an unset one is an error diagnostic, not a
parse failure.

### Versioning and migration

- `codec::read_envelope` classifies every file: not an object/JSON →
  `Malformed`; no version where one is required → `MissingVersion`; newer than
  the build → `FutureVersion`; older than readable → `UnsupportedVersion`.
- `.blendspace`, `.montage`, `.animgraph` read v1 (or unversioned) files and
  migrate in memory. The asset `id` is kept. Sample/segment/section ids that v1
  never had are **derived deterministically** from the asset id, so migrating
  the same bytes twice gives the same ids.
  - Montage v1 → one `DefaultSlot` track; the v1 `start` (offset into the
    clip) becomes `clip_start`; a looping segment becomes `loop_count: 2`;
    section `next_section` names become ids (a link to a missing section is
    dropped and reported); the v1 section `end` is kept as `legacy_end`.
  - Animgraph v1 → `anim.sequence_player` / `anim.blend_space_player` /
    `anim.output_pose` nodes with the **same node ids**; node names become the
    `title` property; each v1 connection becomes a `pose` wire. Connections the
    pose model cannot hold (several into the output, into a player, from the
    output, to a missing node) are dropped, listed in the migration notes and
    kept under `migrated_dropped_connections`.
- `.animseq` keeps writing version 1/2/3 unless a v4 feature is present
  (notify states, payload schemas, or a notify with a `schema`), so untouched
  files are not rewritten.
- The editor/index call `parse_migrating` and only rewrites a file when an id
  had to be assigned; format upgrades happen on the user's next save.
- `.animgraph` is written with all object keys sorted (`codec::to_stable_pretty`)
  because Graphy documents hold hash maps; saves are deterministic and diffable.

### Notify payload schemas and states

`notify_schema.rs`: a `NotifyPayloadSchema` (stable id, typed fields: bool, int,
float, string; defaults; required) is referenced by notifies and notify states
by `schema`. `check` enforces names, types (int widens to float) and required
fields; `resolve` fills defaults, which is the payload an event will carry. A
notify without a schema keeps free-form fields as before. Notifies and states
share one id space inside an asset (the Blueprint descriptor
`animseq::<sequence>.<id>` must name exactly one).

## Animation graph model (the Graphy profile's data)

`anim_graph/catalogue.rs` is the node and pin catalogue the profile renders from
and the Phase 2 compiler lowers: 48 node types, each with pins, typed
properties (`PropKind`), the graph kinds it may appear in, and an
`EvalContract` (evaluation summary, root-motion flow, curve flow, notify source,
statefulness, skeleton need). Pin types (`AnimPinType`): `Pose`,
`AdditivePose`, `Float`, `Bool`, `Int`, `Vector`, `Transform`, `Name`,
`StateFlow`; they map to Graphy `TypeInfo` strings (`anim::Pose`, `f32`, ...),
types connect only when equal (an additive pose never feeds a pose input).

Graph kinds: `Main` (ends in `anim.output_pose`), `Layer`, `StateMachine`
(`sm.entry`, `sm.state`, `sm.transition`), `StatePose` (`anim.state_result`),
`TransitionRule` (`rule.result`). A state machine is a node
(`anim.state_machine`) whose `machine` property names a sub-graph; states and
transitions name further sub-graphs. State machine wiring (Graphy inputs take
one wire): `sm.entry.start → state.default`; `state.leave → transition.from`;
`state.arrive → transition.to`.

Nodes: Sequence Player, Blend Space Player, Pose Search, Reference Pose, Blend,
Blend Multi, Blend by Bool/Int, Make/Apply Additive, Layered Blend per Bone,
Slot, Save/Use Cached Pose, Get Curve, Modify Curve, Two Bone IK, FABRIK, IK
Rig, Retarget Pose, State Machine, Layer, Linked Graph, Output/Layer/State
terminals, parameter and math/logic value nodes, the vector and transform nodes
(Vector, Make Vector, Make Transform, Get Bone Transform: IK effectors need a
producer), and the rule-context nodes
(State Time, Time Remaining, State Complete). `Layer Input` is allowed in a main graph too, so a linked graph can take an input pose.

`anim_graph::validate` reports (all with graph/node location and a repair hint
when one exists): unknown node, wrong graph kind, missing/duplicate terminal,
bad or missing property, stale pins, dangling wires, wrong pin types, multiple
wires into an input, unconnected required inputs, cycles (including through
cached poses), unused nodes, duplicate/missing cached poses, missing or
wrong-kind sub-graphs, layer recursion, state machine topology (no default
state, duplicate names, self transition without re-entry, ambiguous priority,
unreachable states, bad endpoints), parameters, skeleton requirement.
`validate_against_skeleton` checks bone properties, masks and IK chain shape.

## References, authority and compatibility

- Every authored link is an `AssetRef<K>`: a UUID string with a compile-time
  kind. Paths are resolved locations only; reverse references are never stored
  in assets.
- `.skmesh` names its Skeleton; the **component takes the mesh's skeleton**.
  Animations, blend spaces, montages, graphs, rigs and pose databases must be
  for that skeleton.
- `dependencies::extract(kind, bytes)` reads an asset with its real codec and
  lists `AssetEdge { target_kind, target_id, role (hard|hint), via, skeleton
  rule }`. `preview_mesh` is a hint (shown in Used By, ignored by checks).
- Skeleton rules per edge: `SameAsOwner`, `Unconstrained` (skeleton refs
  themselves, IK retargeter rigs), `OwnerSource`/`OwnerTarget` (retarget profile
  rigs; a graph's retarget node needs a profile/retargeter whose **target**
  skeleton is the graph's).
- `dependencies::check_closure(source, kind, id)` walks hard edges and reports
  `dep.missing`, `dep.ambiguous`, `dep.unreadable`, `dep.wrong_kind`,
  `dep.stale_id`, `dep.skeleton_mismatch`, `dep.skeleton_unset`, `dep.cycle`.
- `report::asset_report` = codec + the asset's own diagnostics + closure check:
  the validator the editors and CI use.
- `picker` is the data side of asset pickers: `describe` an asset file,
  `filter` candidates by kind and the owner's skeleton (hide or flag
  incompatible). The picker UI is Phase 4.

### The index (`engine_fs::ProjectFileIndex`)

Schema version 4 adds `asset_deps(source_path, source_kind, source_id,
target_kind, target_id, role, via, expect)`; rows cascade-delete with the file
row and follow `move_path`. API: `replace_asset_deps`, `clear_asset_deps`,
`asset_deps_of`, `asset_used_by` (the "Used By" query), `assets_of_kind`.
`skm_editor::core::asset_index` records an asset's id and edges whenever the
skeleton/mesh/animation editors or libraries open or save a file and when the
high-level asset editor opens or saves; `ProjectAssetSource` resolves
`(kind, id)` through the index for the validators (missing / ambiguous /
wrong-extension are distinct results).

## Runtime (Phase 2)

`skeletal_assets::runtime` turns validated assets into poses. It is pure,
headless and deterministic (no wall clock, no hash-map iteration order in any
result). The component (`helio_component::SkeletalMeshComponent`) owns the only
mutable state.

| Module | What |
|---|---|
| `pose` | `BoneTransform` (TRS), `Pose` (bones + curves + root motion + additive kind), `RuntimeSkeleton` (index based, topological order), mesh/local conversion, additive make/apply, weighted blend |
| `clip` | `CompiledClip`: per-bone quaternion tracks, curves, loop-seam root motion, root-motion removal from the pose |
| `blendspace` | 1D/2D solving; 2D uses a Bowyer-Watson Delaunay built once from samples sorted by `(x, y, index)` in axis-normalised coordinates; outside the hull the input is projected to the nearest edge; collinear samples fall back to the line; weights are non-negative and sum to 1 |
| `ik`, `ikrig` | analytic two-bone (pole, stretch, limits) and FABRIK (precision, iterations, limits); IK Rig = ordered solver stack against goals with per-goal position/rotation alpha |
| `retarget` | one plan for `.animretarget` (by bone or by chain) and `.ikretarget` (FK per chain + IK goals through the target rig): mesh-space rotation deltas from the source retarget pose applied on the target retarget pose, translation modes, root policy and scale |
| `posesearch` | deterministic index build from a `.posedb` (root-frame pose and trajectory features, standardisation), brute-force nearest query with a per-channel cost breakdown, continuing-pose bias, name-based left/right mirror table, binary payload for the derived cache |
| `montage` | `CompiledMontage` and `MontageSet`: play/stop/pause/rate/priority/jump/set-next-section, blend in/out, group interruption policy, deterministic one-shot results |
| `graph` | `compile` (validate → resolve assets → immutable `Program`s), `instance` (update/evaluate passes, sync groups), `machine` (state machines) |
| `anim_instance` | `AnimInstance`: parameters, Montages, evaluated and previous pose, fixed/variable stepping, scrubbing, hot-reload adoption |
| `playback` | `NotifyTracker`: one playback instance per Sequence Player, Blend Space sample, Pose Search cursor, Montage segment and Montage timeline; notifies and notify-state begin/tick/end (Phase 3) |
| `rootmotion` | `RootMotionMode`, `RootMotionPolicy` and the owner transform math (Phase 3) |
| `memory` | `MemorySource`: an in-memory `AssetSource` for tests, tools and hosts that already hold their assets |

### Ownership

- **Immutable and shared:** `CompiledGraph`, its `Program`s, `CompiledClip`,
  `CompiledBlendSpace`, `CompiledIkRig`, `RetargetPlan`, `CompiledPoseDb`,
  `CompiledMontage`. Two components playing the same graph hold the same `Arc`.
- **Per component:** `AnimInstance` inside `SkeletalMeshComponent` (never
  serialized, never cloned: a duplicated component starts its own playback) —
  parameters, node state (clocks, cross-fades, state machine states, Pose Search
  cursor and query history), `MontageSet`, evaluated pose, previous pose, curves,
  root-motion delta of the latest advance, and the notify outbox.
- **Not in assets:** nothing runtime is ever written to `.animgraph`,
  `.montage` or any other asset.

### Compilation

`compile_graph(asset, &dyn AssetSource)` runs `validate` first; any error
returns diagnostics and **no graph** (all-or-nothing). Everything a node names is
resolved at compile time: sequences, blend spaces (and their samples), IK Rigs,
retarget profiles/retargeters, pose databases, linked graphs, bone names and
masks. Failures are diagnostics with `graph/node` locations and a repair
(`graph.sequence_unavailable`, `graph.skeleton_mismatch`, `graph.mixed_skeleton`,
`graph.linked_cycle`, `graph.ik_goal`, `graph.pose_search_unavailable`, ...).
The one runtime fallback the catalogue promises is Pose Search playing its
`fallback` input when the database cannot be used; it is accepted only when a
fallback pose is connected, and then compiles with
`graph.pose_search_fallback`.

Nodes upstream of a Retarget `source` input are over the profile's *source*
skeleton: sequences there are checked against it (not the graph's) and a node
that would feed both skeletons is `graph.mixed_skeleton`. Dependency checks
(`dependencies::extract`) treat those edges as `Unconstrained` for the same
reason (`validate::retarget_source_nodes`).

`ProjectAnimSource` (`helio_component::anim_source`) is the project-backed
`AssetSource`: ids resolve through the file index and files are read with each
asset's own codec. The derived Pose Search index is stored at
`.pulsar/cache/posedb/<db id>-<hash16>.bin` keyed by the database's settings and
the content hash of every source sequence (`PoseDbAsset::index_key`), written
atomically, and rebuilt when the key changes.

### Evaluation: two passes per update

1. **Update** decides weights and advances clocks. Only contributing nodes
   (weight > 0) are touched; a branch with no weight does not advance, and a
   player that becomes relevant again restarts from its `start_position`. Value
   nodes that read a pose (Get Curve, Get Bone Transform) return the *previous*
   update's value here, because weights must be known before poses can be
   sampled. Sync-grouped players register instead of advancing.
2. **Sync resolve** (only if a group registered): the leader is the highest-weight
   explicit leader, else auto member, else follower (ties: lower node key); its
   normalised phase becomes the group's; followers land on the same phase.
3. **Evaluate** samples poses from the stored decisions. Every node is evaluated
   at most once per update however many wires read it (cached poses are a
   consequence, not a special case); value nodes see the fresh pose.

Semantics worth knowing:

- Sequence `looping: inherit` means **loop** (a sequence asset carries no loop
  flag). `once` holds the last frame; the `State Complete` rule needs `once`.
- Playback rate may be negative in Sequence Player and Blend Space nodes (reverse
  playback, see "Reverse playback" below). On the component, `playback_rate` scales
  time by its size; its sign only picks the direction of the internal single-sequence
  player.
- Blend space axis smoothing: `k = 1 - exp(-3·dt/τ)`, so the input reaches ~95%
  of a step in `smoothing_time`. `SyncMode::Phase` shares one phase across all
  contributing samples, the cycle being the weight-averaged (or leader's) length;
  `Markers` phase-aligns only samples that name a `sync_group` (the asset carries
  no marker positions); ungrouped samples run on their own clocks.
- Additive: local additive is `t += Δt; r = base.r · Δr; s *= Δs`; mesh additive
  applies `Δr · base_mesh.r` in mesh space. Apply scales by alpha; curves add.
- Layered Blend per Bone: bone weight = mask weight × layer weight; a layer
  without a mask drives the whole body; `mesh_space_rotation` blends rotations in
  mesh space. Curve ownership follows `curve_blend` (`base`, `layer`, `max`,
  `sum`); root motion follows `root_motion_from`.
- IK effectors are `Transform` values built by `Make Transform` (location + Euler
  degrees XYZ) or `Get Bone Transform`; `effector_space: bone` offsets from the
  end bone's current mesh transform. Mesh space is Y-up.
- Pose Search: brute force over all indexed entries; `candidate_count` entries
  are reported, ties break to the lower entry. Past trajectory offsets assume the
  character has been moving as the query says. Mirrored sources use the
  left/right table derived from bone **names** (`Left/Right`, `_L/_R`, `.L/.R`,
  `L_/R_`); unpaired sided bones are reported by `MirrorTable::unpaired`.

### State machines

States hold live graph instances. A rule is evaluated (before the state
advances) with the source state's time, the primary player's remaining time and
completion. The first true rule of the allowed transitions fires, ordered by
`(priority, node id)`. Firing starts a blend **from the weights the machine has
right now** to the target, so an interrupting transition mixes rather than
snaps. A transition in flight allows new transitions from its source states
(`interrupt: source|both`) and/or its target (`target|both`); `none` lets it
finish. A transition from a state to itself needs `allow_reentry` and starts a
fresh instance while the old one fades. `reset_on_entry: false` keeps a left
state's instance dormant and resumes it on re-entry (its time still restarts).
A machine that becomes relevant after a gap restarts at its default state;
after a graph hot reload it resumes in the same-named state.

### Montages

`MontageSet` (per component): `play(montage, rate, section)` applies the
montage's group policy first — `ReplaceGroup` interrupts the group's instances
(they report `Interrupted { by }`), `RejectIfBusy` and `StartIfHigherPriority`
return `PlayRefused`, `Overlap` plays alongside. Each instance reports exactly
one `MontageResult` (`Completed`, `Interrupted`, `Stopped`, `Cancelled`) plus
`Started`, `SectionChanged`, `BlendingOut` events. A section's `next: None` ends
the montage at that section's end; `set_next_section` overrides links for one
instance; `jump_to_section` and `seek` raise nothing for skipped time.

A Slot node `(slot, group)` overlays every instance with a track on that slot:
lower priority first, ties by start order, so the highest priority ends on top.
Track masks limit the bones driven; a full-body track at full weight stops the
source being updated (unless `always_update_source`). Curves the montage supplies
override the graph's scaled by weight; root motion follows the montage's setting
(`montage` replaces by weight, `graph` leaves the graph's, `ignore` blends the
graph's toward zero).

### Component and engine integration

- `SkeletalMeshComponent.animation_graph` is the one authored animation source.
  The old direct `animation` + `looping` keys are **compatibility for one
  release** (D9): they decode into the internal single-sequence player — a
  compiled one-node graph (`single_sequence_graph`) inside the same
  `AnimInstance`, no `.animgraph` file — and are written back under the same keys
  so no scene loses its animation. They are hidden from the property panel. A
  graph shadows the old sequence (`component.sequence_shadowed_by_graph`).
  `play_sequence` sets the internal player from code.
- **Frame order** (`pulsar_game::TickLoop::run_tick`): input flush → **animation
  phase** (`helio_component::run_animation_phase`) → ECS schedule (physics reads
  `bone_world_pose`) → component lifecycle tick → actors → physics-pose writeback
  → `AfterPhysics` flush → scripts → **notify delivery**
  (`flush_animation_notifies`, class-variable targets resolved through
  `ScriptDriver::entity_variable`) → `AfterScripts`/`EndOfFrame` flush. Parameters
  and Montage commands written by scripts or Blueprints (phase 3 of the frame)
  apply in the **next** frame's animation phase.
- **Authority:** animation drives the skeleton. The physics rig owns the bones its
  dynamic bodies drive (`apply_physics_bones`; the animation keeps running, see
  "Physics authority"). Whole-pose ownership via `apply_physics_pose` holds the
  animation clock; `clear_physics_pose` returns ownership and republishes the
  animation's pose.
  The component is the only writer of its GPU palettes (current + previous in
  `gpu_bone_palettes`), written under the scene write lock.
- Time: pause is `playing = false` (or a zero delta); `playback_rate` scales;
  `AnimInstance::advance` supports `TimeStep::Fixed { step, max_substeps }` (the
  remainder carries, excess time is dropped) and `UpdateMode::Scrub`, which
  reports no playback spans or Montage events. Seeking the single-sequence player
  never raises notifies. A component that is disabled or loses its owner tears
  its instance down (Montages report `Cancelled`) and starts fresh when re-enabled.
- Gameplay API on the component: `set_animation_float/bool/int/vector`
  (validated name, type, finiteness), `play_montage` (compiles and caches per
  skeleton) / `play_compiled_montage`, `stop_montage[_group]`, `pause_montage`,
  `set_montage_rate`, `jump_to_montage_section`, `set_montage_next_section`,
  `take_montage_events`, `animation_trace`, `animation_root_motion`.
- Notifies: every playback the instance reports (graph players, blend space
  samples, Pose Search, Montage segment clips and Montage timelines) goes through the
  component's `NotifyTracker`; notifies and notify-state phases are published with
  the owning asset's canonical descriptor (see Phase 3 below).
  Blueprint discovery (`component_animation_notify_events`) resolves the
  component's graph closure through the asset index — Sequence Players, blend
  space samples, pose database sources, linked graphs and the sequences of
  Montages that have a track on one of the graph's Slots — without a project walk.

## Not yet built

- The animation Graphy profile (presenter, catalogue UI, state-machine view) and
  the purpose-built editors: Phase 4. `animation_assets_editor.rs` is a typed
  interim editor (pickers, diagnostics, Used By), not a generic JSON form.
- Bone/curve LOD and update-rate controls (Phase 5, with profiling).

## Tests

`cargo test -p skeletal_assets` (codecs, migration, validators, dependency
closure, fixtures in `skeletal_assets/tests/fixtures`; `REGEN_FIXTURES=1`
rewrites the generated `v2/` files; and the runtime: `runtime::` unit tests for
pose math, clips and root motion, blend spaces, IK, IK Rig, retarget, Pose Search,
Montages, and `runtime::graph::tests` for every node family, state machines,
slots over Montages, sync groups, fixed stepping, scrubbing, hot reload, linked
graphs and layers, diagnostics, the typed parameter API and instance
independence), `cargo test -p engine_fs --lib asset_dep`,
`cargo test -p pulsar_reflection --lib animation_kind`,
`cargo test -p skeletal_animation_plugin --lib` (registration),
`cargo test -p helio_component --lib components::skeletal` (component links, the
notify path, D9 migration, graph components, Montages through the component,
teardown), `cargo test -p pulsar_game --lib animation_phase_order` (the
animation phase runs before the schedule). Phase 3: `cargo test -p skeletal_assets --lib`
(`runtime::playback` tracker and Montage-driven notifies, reverse graph players and
Montages, root motion modes), `cargo test -p helio_component --lib components::skeletal`
(`skeletal_phase3_tests.rs`: Montage events through the component, two components with
shared assets, teardown, hot reload from disk, Blueprint discovery and nodes, root
motion across loop seams, bone-level physics ownership), `cargo test -p blueprint_compiler
--test compile`, the plugin's `animation_event_tests` and `cargo test -p engine_fs --lib
animation_update`.

## Notify, event, root-motion and physics semantics (Phase 3)

Everything here is headless and deterministic; `skeletal_assets::runtime::playback`
and `::rootmotion` are pure, `helio_component::SkeletalMeshComponent` wires them to
the engine.

### Decisions in force (Phase 3)

| # | Decision |
|---|---|
| D12 | A **playback instance** is one clock playing one asset timeline: a Sequence Player node, one contributing Blend Space sample, a Pose Search cursor, one segment of one Montage instance, or a Montage instance's own timeline. It is keyed by `(owning graph/Montage instance, position in the graph)`, so two components, two Montage instances or two nodes on one clip never share notify state. |
| D13 | A playback that is not reported in an update (weight zero, node left the graph, Montage ended/interrupted/stopped) is over; a later return is a **new** playback instance. |
| D14 | Continuity is judged by the tracker: a span whose `from` is not where the previous one ended is a **jump** (seek, section jump or link, Pose Search switch). Looping spans that only restart their cycle count (Pose Search) are rebased, not jumps. |
| D15 | Pause, scrub and fixed-step remainders are *no update*: nothing begins, ticks or ends. A preview scrub (`UpdateMode::Scrub`) reports no spans at all, so it cannot raise gameplay events. |
| D16 | `root_motion_mode` defaults to `IgnoreRootMotion` (extract and discard), which is exactly what scenes saved before root motion was applied did. |
| D17 | Physics owns **bones**, not the whole skeleton: `apply_physics_bones` replaces configured bones in the drawn palette; the animation clock never stops for them. Whole-pose ownership (`apply_physics_pose`) remains and holds the clock. |

### Playback instances and notify delivery

`NotifyTracker` (per component, never serialized or cloned) turns the spans one
advance reports (`UpdateReport::spans`, `montage_timelines`, `finished_montages`) into
`NotifyOccurrence`s:

- **Notify** fires when the playhead passes it (departure excluded, arrival
  included), forward or reverse, across any number of loops.
- **Notify state** is entered when the playhead is inside `[start, end)` (reverse:
  `(start, end]`). Each activation raises `Begin` once, `Tick` per update while inside
  (or per `tick_interval`, carrying the time played inside as `delta`), and `End` once
  with a reason. A state whose range lies inside one big step raises
  Begin, Tick, End in that one update; looping gives one activation per cycle (`cycle`
  tells which); a `disabled` state is silent.
- Order inside one span: distance travelled along the playhead path, then
  Tick < End < Begin < Notify, then loop cycle in playback direction, then the asset's
  own order. The order across playbacks is the key order (deterministic). Work per span
  is bounded: more than a million (state x cycle) pairs or notify crossings is an error
  and nothing from that span is reported.

| situation | `end_reason` of active states |
|---|---|
| playhead leaves the range (any direction, wrap, big step) | `completed` |
| clock jumps (section jump/link, seek without telling the tracker, Pose Search switch) | `jump` |
| owner seeks / restarts (`seek_animation`, `restart_animation`, `stop_animation`) | `seek` |
| playback stops contributing, Montage ended/interrupted/stopped | `stopped` |
| the asset behind the playback was replaced (graph/sequence/Montage hot reload, mesh or skeleton rebind) | `reload` |
| component disabled or its owner removed (owner still alive) | `teardown` |

A component whose owner has been despawned drops its queue: there is nothing left to
deliver to.

**Sources of notifies.** Sequences own `notifies` and `notify_states` (as before).
Montages now own theirs too (`AnimationMontageAsset::notifies/notify_states`, on the
Montage timeline) and compile them into `CompiledMontage`. A Montage instance reports
its timeline and one playback per segment clip per loop cycle, so a Montage raises its
own events plus those of the clips it plays.

**Delivery** goes through the owning component only (no global publication):

- `SkeletalMeshComponent.animation_notify` (`AnimationNotifyEvent`): every notify and
  every state phase, with `sequence_id` (the owning asset's id whatever its kind),
  `source_kind` (`animseq`/`montage`), `notify_id`, `notify_name`, `clip_time`,
  `playback_time`, `cycle`, `generation`, `playback_instance`, `notify_phase`
  (`notify|begin|tick|end`), `tick_delta`, `end_reason`.
- Canonical per-asset events on the target entity channel (the owner object, or the
  class variable the notify names): `animseq::<sequence id>.<notify or state id>` and
  `montage::<montage id>.<id>`; fields are the common ones above (with `entity` and
  `source` first: target and owner) followed by the authored typed payload.
- Montage lifecycle, both as component events `SkeletalMeshComponent.montage_started`,
  `montage_blended_in`, `montage_section_changed`, `montage_blending_out`,
  `montage_ended`, `montage_interrupted` (`MontageLifecycleEvent`: `montage_id`,
  `instance`, `section_from/to`, `result` = completed|interrupted|stopped|cancelled,
  `interrupted_by`) and as canonical `montage::<montage id>.lifecycle_<kind>` events.
  `ended` fires exactly once per instance; an interruption also raises `interrupted`.
  Within one advance: started, blended in, section changed, then notifies, then
  blending out and ended.
- A failed publication retries the unaccepted suffix next frame; the animation holds
  while events wait.

Descriptors are registered when events are first delivered and on every tick for
playing Montages; Blueprint discovery registers nothing on a bus.

### Blueprint

Discovery (`component_animation_notify_events`) reads the component's closure through
the asset index on every call (no per-open cache): Sequence notifies **and states** of
every sequence the graph can play, and for each Montage with a track on one of the
graph's Slots its own notifies, states and six lifecycle events. The compiler and
palette treat `animseq::...` and `montage::...` identically: a node is bound to one
`SkeletalMeshComponent` reference through `component_ref`, filtered to its asset by its
canonical name, and never a global broadcast. The graph asset itself raises no events of
its own (state machines are traced, not evented).

Nodes (world methods of the component, so they appear in the palette and TypeScript
declarations like every other component method; they take the component reference or
its object): `anim_set_float/bool/int/vector` (graph parameters: unknown name, wrong
type, non-finite value are errors), `anim_play_montage` (Montage asset id or unique file
name, rate, section; returns an instance handle; the group policy may refuse),
`anim_stop_montage`, `anim_stop_montage_group`, `anim_pause_montage`,
`anim_jump_to_section`, `anim_set_next_section`, `anim_set_montage_rate` (negative =
reverse), `anim_is_montage_playing`. Commands apply in the next animation phase, in call
order.

### Reverse playback

- **Sequence Player / Blend Space nodes**: a negative `play_rate` plays backwards. A
  looping clip wraps below zero; a one-shot holds its first frame. Sync-grouped nodes
  and phase-synced Blend Space samples need forward time: a negative rate leaves the
  group and the node runs on its own clock (the phase of a phase-synced space runs
  backwards). Root motion is the inverse of the forward motion over the same stretch.
- **Montage**: a negative rate plays the timeline backwards inside the current section;
  at its start the playhead continues into the section that links *to* it (itself if it
  loops, else the first such section in asset order) and with none the Montage
  completes. Notifies and states mirror the forward order.
- **Component**: `playback_rate` < 0 plays the internal single-sequence player
  backwards (graph nodes and Montages carry their own rates).

### Root motion

Extraction happens in the instance (see Evaluation); the component applies it:

| `root_motion_mode` | extraction | the owner moves by |
|---|---|---|
| `NoRootMotionExtraction` | none, the root bone moves in the pose | nothing |
| `IgnoreRootMotion` (default) | yes | nothing, delta dropped |
| `RootMotionFromEverything` | yes | graph players and Montages as blended |
| `RootMotionFromMontagesOnly` | yes | only Montages in `Montage` root-motion mode (`montage_root_motion`) |

`root_motion_rotation` (`NoRotation`, `YawOnly` default, `FullRotation`) and
`root_motion_vertical` (default off: height belongs to gravity and ground contact)
constrain the delta. The delta is composed on the right in the owner's **local** frame,
translation scaled by the owner's scale, written to the owner's `Transform` once per
frame in the animation phase (before the ECS schedule, so physics and everything after
read the moved owner). Moves go through the same `MotionGate`s as script moves: a
`Static` object is not moved and the delta is dropped. Montage precedence per Montage:
`montage` replaces the graph's by blend weight, `graph` leaves it, `ignore` blends it
toward zero. Loop seams compose exactly (the clip's per-cycle motion is added whole), a
Montage's section links and jumps contribute only the time actually played (the
playhead is reported in continuous runs), and slicing time differently gives the same
total travel. A dynamic rigid body owner should use `Ignore`/`NoRootMotionExtraction`
and let physics move it: the animation phase runs first and physics wins afterwards.
Multiplayer authority is not modelled: root motion is applied by whoever runs the
component's animation phase.

### Physics authority

Per frame: animation phase (evaluate; `bone_world_pose` = the animation's own pose,
what physics reads) then physics step and write-back, then `apply_physics_bones` (the
drawn palette takes the rig's dynamic bones, their descendants follow them, everything
else stays animated). The component is the only writer of the palette and of
`bone_world_pose`; the rig writes through the component in write-back, sequentially,
so there is no race. `skeletal_rig` reports only dynamic-body bones; kinematic bodies
keep following the animation. An empty list gives every bone back.

### Hot reload

`AssetUpdated(Custom("animation"))` is published by the file watcher for
`.skeleton .animseq .blendspace .montage .animgraph .ikrig .ikretarget .animretarget
.posedb`. `engine_backend::scene::reload_updated_asset` reloads the graph/sequence a
component holds directly through SceneDB and calls
`helio_component::reload_animation_dependencies`: every component whose compiled
graph's `dependencies` (resolved to canonical paths when the graph binds) include the
file recompiles its graph from disk and swaps it in place (`set_graph` adoption keeps
clocks, state machine states and parameter values of surviving nodes); active notify
states end with `reload`. A graph that no longer compiles keeps running as it was and
reports diagnostics. Montage compiles are cached per component and dropped on any
animation change; an instance already playing finishes with the version it started
with.

### Not covered / limits

- Notify state ticks are per update, not per `tick_interval` sub-step inside an
  update; a big step gives one tick with the whole `delta`.
- Trimmed *looping* Montage segments restart their clip range every cycle (a
  discontinuity: states end and begin per cycle; a notify at exactly the cycle start is
  not re-fired at the restart).
- Whole-skeleton physics ownership (`apply_physics_pose`) still holds the clock.
- Server/client authority for root motion.
