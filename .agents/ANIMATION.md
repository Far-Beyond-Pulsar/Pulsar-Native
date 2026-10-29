# Animation assets, references, compatibility and runtime (Phases 1–2)

How the animation family of assets is stored, linked and checked (Phase 1), and
how they run (Phase 2): the compiled graph, per-component instance state, state
machines, Montages, IK, retargeting, Pose Search and the engine integration.
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
- Playback rate is non-negative in graph nodes; negative `playback_rate` on the
  component holds the pose (reverse playback of root motion exists in the clip
  sampler and Montages with negative rate are refused, `PlayRefused::InvalidRate`).
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
- **Authority:** animation drives the skeleton. When the physics rig calls
  `apply_physics_pose` it owns the pose and the animation clock is held;
  `clear_physics_pose` returns ownership and republishes the animation's pose.
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
- Notifies: every sequence span the instance reports (graph players, blend space
  samples, Pose Search, Montage segment clips) is turned into crossings through
  the clip's own notify index and published with the sequence's descriptor.
  Blueprint discovery (`component_animation_notify_events`) resolves the
  component's graph closure through the asset index — Sequence Players, blend
  space samples, pose database sources, linked graphs and the sequences of
  Montages that have a track on one of the graph's Slots — without a project walk.

## Not yet built

- The animation Graphy profile (presenter, catalogue UI, state-machine view) and
  the purpose-built editors: Phase 4. `animation_assets_editor.rs` is a typed
  interim editor (pickers, diagnostics, Used By), not a generic JSON form.
- Phase 3: Montage-authored notifies and notify **states** (begin/tick/end),
  component-scoped delivery of Montage lifecycle events as Blueprint events,
  root-motion application to the owner's transform and its physics/network
  authority, Blueprint nodes for parameters and Montage commands.
- Dependency hot reload: editing a *source asset a graph names* (a sequence, blend
  space, rig, ...) does not recompile running graphs by itself; saving the graph,
  re-binding the mesh or reloading the scene does. `CompiledGraph::dependencies`
  lists what to watch.
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
animation phase runs before the schedule).
