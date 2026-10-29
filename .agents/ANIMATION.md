# Animation assets, references and compatibility (Phase 1)

How the animation family of assets is stored, linked and checked. Runtime
evaluation (graph, Montage, IK, retarget, Pose Search execution) is Phase 2 and
later; this document describes what is built: the schemas, the typed stable-id
references, the dependency index and the validators. Plan:
`docs/animation-system-implementation-plan.md`; audit:
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
| D9 | The direct `animation` ref on the component stays the authored single-sequence source for one release. Moving it into an internal player state happens with the graph runtime (Phase 2); no second runtime is introduced. |
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
and the Phase 2 compiler lowers: 44 node types, each with pins, typed
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
terminals, parameter and math/logic value nodes, and the rule-context nodes
(State Time, Time Remaining, State Complete).

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

## Not yet built

- The animation Graphy profile (presenter, catalogue UI, state-machine view) and
  the purpose-built editors: Phase 4. `animation_assets_editor.rs` is a typed
  interim editor (pickers, diagnostics, Used By), not a generic JSON form.
- Evaluation of any node, the Montage runtime, IK/retarget execution, Pose
  Search build and query, notify-state dispatch: Phases 2–3. The Pose Database
  derived index has its key, header codec and cache location (`.pulsar/cache/
  posedb/<id>-<hash16>.bin`) defined; nothing builds one yet.
- Blueprint discovery still walks the project for sequence notifies and filters
  on `SkeletalMeshComponent.animation`; moving it to the dependency closure is
  Phase 3.

## Tests

`cargo test -p skeletal_assets` (codecs, migration, validators, dependency
closure, fixtures in `skeletal_assets/tests/fixtures`; `REGEN_FIXTURES=1`
rewrites the generated `v2/` files), `cargo test -p engine_fs --lib asset_dep`,
`cargo test -p pulsar_reflection --lib animation_kind`,
`cargo test -p skeletal_animation_plugin --lib` (registration),
`cargo test -p helio_component authority` (component links).
