# Animation system — Phase 0 audit (baseline and ownership)

Branch `animation-phase0`, cut from `main` @ `381ea0454`. Research and documentation
only: no code, manifest or pin was changed, and **no measurement was run** (the user
asked to skip test/benchmark/baseline runs for now). Everything below comes from reading
the live checkout; claims that need a run to confirm are marked *(unverified at runtime)*.

Plan under audit: `docs/animation-system-implementation-plan.md`. Mockup:
`docs/mockups/animation-editor.html`.

## 0. Checkout and submodule state

| Repo (path) | Pinned commit | Notes |
|---|---|---|
| `plugins/vendor/skm_editor` | `c509b95` (main) | crate `skeletal_animation_plugin` + member crate `skeletal_assets` |
| `crates/renderer/helio` | `4ca90aa8` (v0.20.0-140) | `helio-component`, `helio-asset-compat` |
| `crates/external/pulsar-reflection` | `48b85ed` (main) | `asset_refs.rs` |
| `crates/third-party/graphy` | `6c3da62` (main) | no animation profile exists (see §5) |
| `plugins/vendor/blueprint_editor` | `5016fa8` | notify events + compiler |
| `plugins/vendor/physics_plugin` | `3b16b71` (main) | `skeletal_rig.rs`, `.physasset` |
| SceneDB (cargo git dep) | rev `017252c3…` | not a submodule; checkout in `~/.cargo/git/checkouts/scenedb-*/017252c` |

Working tree was clean on `main`; the plan's *docs* files are tracked on `main` (nothing
untracked was present). Docs the plan tells Phase 0 to read: `POLISH.md` does **not**
exist at the repo root. The only one is `plugins/vendor/skm_editor/POLISH.md`, and it is
2 bytes (empty). The skeleton-hierarchy doc is `plugins/vendor/skm_editor/docs/skeleton-hierarchy.md`.
`.agents/` has no animation/skeletal doc beyond a paragraph in `CRATES.md` (importer).

## 1. Serialization schema and ownership table

### 1.1 Registrations (the real extensions)

All six file types are registered in `skm_editor/src/lib.rs` (`file_types()`), editors in
`editors()`/`register_editors()`, and wired into the host through
`crates/editor/ui_core/src/builtin_editors.rs:329-360` (a generic wrapper over
`SkeletalAnimationPlugin::{file_types,editors,create_editor}`; it needs no edit per asset
type, only the `provides_editor` id list in `skm_editor`).

| Extension | FileType id | Editor id | Reflection `AssetKind::KIND` | `EXTENSIONS` (accepts) | Creatable from menu |
|---|---|---|---|---|---|
| `.skeleton` | `skeleton` | `skeleton-editor` | `skeleton` | `skeleton` | yes |
| `.skmesh` | `skeletal_mesh` | `skeletal-mesh-editor` | `skmesh` | `skmesh` | no (import only) |
| `.animseq` | `animation_sequence` | `animation-sequence-editor` | `animseq` | `animseq` | yes |
| `.blendspace` | `blend_space` | `blend-space-editor` | `blendspace` | `blendspace` | yes (`default_content` JSON) |
| `.montage` | `animation_montage` | `animation-montage-editor` | `anim_montage` | **`montage`, `anim_montage`** | yes |
| `.animgraph` | `animation_graph` | `animation-graph-editor` | `anim_graph` | **`animgraph`, `anim_graph`** | yes |
| `.skelanim` (folder, legacy) | `skeletal_animation` | opens in sequence editor | — | — | no (converted by `core/migrate.rs`) |
| `.physasset` (physics_plugin) | — | physics rig editor | — | — | n/a; references `SkeletalMeshId` + bone ids |

**`.montage` vs `.animmontage` — resolved.** The registered file type, the saved extension
(`MONTAGE_EXTENSION = "montage"`), the editor, and the Reflection `EXTENSIONS` all say
`montage`. `.animmontage` appears in exactly one place: the mockup (line 202,
`Rifle_Reload.animmontage`). No repo file, test or fixture uses `.animmontage`, and no
`.montage` file is committed anywhere. **Recommendation: canonical `.montage`;** change the
single mockup string; no alias or migration is needed (nothing to migrate). Separately,
`AnimationMontageAssetKind::EXTENSIONS` also accepts `anim_montage` and
`AnimationGraphAssetKind::EXTENSIONS` accepts `anim_graph`: those are *kind-string spellings
leaked into the extension list*, registered by no file type. Recommend dropping them in
the Reflection change of Phase 1 (they make two spellings "valid" for pickers). Do **not**
rename `KIND` (`anim_montage`, `anim_graph`): editors write it into the file-index
`asset_ids.kind` column, so changing it invalidates recorded ids.

### 1.2 Codecs, versions and ownership

| Asset | Codec location | Encoding | Version | Unknown-field policy | Verdict |
|---|---|---|---|---|---|
| `.skeleton` | `skeletal_assets/src/assets.rs` (`SkeletonAsset`, `load_/save_skeleton_asset`) | JSON, atomic write via `engine_fs::virtual_fs` | `FORMAT_VERSION = 1`; newer refused | **dropped on save** (no extras map) | **Extend**: add nothing structural; add `used-by` queries; bone metadata only if needed by IK/retarget |
| `.skmesh` | `skeletal_assets/src/skmesh.rs` | binary: `PSKM` magic + u32 version + JSON header + vertex payload | `SKMESH_VERSION = 1`; newer refused | header unknowns **dropped** | **Keep** (binary payload must survive). Skeleton ref is the authority; see §2.1 |
| `.animseq` | `assets.rs` (`AnimSeqAsset`), `animation.rs`, `notifies.rs`, `curves.rs`, `root_motion.rs`, `sequence_features.rs` | JSON (clip flattened with `AnimationFeatures`) | `ANIMSEQ_FORMAT_VERSION = 3`; writer picks version lazily: 1 (no features) / 2 (features) / 3 (notify target or fields); newer refused | unknowns **dropped** | **Extend**: notify *states* (no duration field today), typed payload, additive/ref pose metadata, sync markers. Bump to v4 |
| `.blendspace` | `skeletal_assets/src/animation_sets.rs` | JSON | `FORMAT_VERSION = 1` | dropped | **Extend/redefine** (v2): axes policy, per-sample sync/rate, triangulation settings |
| `.montage` | `animation_sets.rs` | JSON | 1 | dropped | **Extend/redefine** (v2): slots/tracks, blend in/out, sections, notifies, interruption policy |
| `.animgraph` | `animation_sets.rs` | JSON (`nodes` + `connections`, string ids, only Sequence/BlendSpace/Output) | 1 | dropped | **Replace schema** with a Graphy `GraphDescription`-based v2; migrate v1 |
| `.physasset` | `pulsar_physics/src/skeletal.rs`, `physics_plugin/src/assets/rig.rs` | JSON, `extra` map keeps unknowns | `SCHEMA_VERSION = 1` | **preserved** | **Keep**; it is the pattern to copy for unknown-field survival |
| `.skelanim` | `skm_editor/src/core/migrate.rs`, `core/serialization` | folder with `skeleton.json`+`animation.json` | legacy | n/a | **Keep** until Phase 5, then decide deletion |

Facts that matter for migration:

- **No codec preserves unknown fields** except `.physasset`. The plan's "unknown supported
  fields survive" therefore needs new `extra` maps (or typed v2 fields) in every rewritten codec.
- The three high-level codecs share one macro (`asset_io!`) and one `parse_versioned` that
  only rejects `version > 1`; there is no migration step and no `v0` handling.
  `id: AssetRef<…>` is `#[serde(default)]` → a file without `id` gets a **fresh random id on
  every parse**; the editor (`animation_assets_editor.rs::open`) rewrites such files once
  (`needs_save`), but any other reader (a loader, an index scan) would see an unstable id.
  v2 codecs must treat missing id as an error or as "assign once and save", never default-random.
- Mixed id types: `.skeleton/.skmesh/.animseq` use `skeletal_assets::ids::{SkeletonId,
  SkeletalMeshId, AnimSequenceId}` (string newtypes); `.blendspace/.montage/.animgraph` use
  `pulsar_reflection::AssetRef<K>`. Both serialize as a UUID string, so they are
  wire-compatible; the plan's "typed AssetRef everywhere" is a Rust-type unification, not a
  file-format change. `AnimSeqAsset.skeleton` is `Option<SkeletonId>`; the mesh's is
  `Option<SkeletonId>`; the component's is `Option<AssetRef<SkeletonAssetKind>>`.
- `AnimationNotify` is `{id, name, time, target, fields}`: instantaneous events only. The
  plan's "notify states with begin/tick/end" requires new schema. `NotifyTarget` already
  stores a class-variable *name*, not an entity handle (good, matches the plan).
- Round-trip tests exist for `.skeleton`, `.skmesh`, `.animseq` (see §6). **No test exists
  for `.blendspace`, `.montage` or `.animgraph`** beyond the editor's file-type table (and
  `lib.rs`'s registration test does not even list those three).

### 1.3 Reverse dependencies on `skeletal_assets`

`skeletal_assets` lives *inside* the `skm_editor` repo and is a Pulsar workspace member
(`Cargo.toml:22,163`). Dependents (Cargo): `asset_import`, `helio-component` (Helio),
`pulsar_physics`, `physics_plugin`, `skm_editor` itself. Source-level users:

- `crates/core/asset_import/src/rig.rs` + tests `rigged_import.rs`, `real_sample.rs`
- `helio-component/src/components/{skeletal_mesh_component,skeletal_notifies}.rs`
- `crates/subsystems/pulsar_physics/src/skeletal.rs`, `physics_plugin/src/{assets,editors}/rig.rs`
- `skm_editor/src/**` (panel, library, validate, hierarchy*, viewport, timeline, properties)

Consequence: any change to `skeletal_assets` public types ripples through **four repos**
(skm_editor, Helio, physics_plugin, Pulsar-Native) — see the order in §6.2.

## 2. Reference and dependency DAG

### 2.1 Today (as built)

```text
SkeletalMeshComponent (SceneDB row; authored: mesh, skeleton, animation, playing, looping, playback_rate)
  ├── mesh      AssetRef<SkeletalMesh>   ──► .skmesh ──(SkeletonId, raw)──► .skeleton
  ├── skeleton  AssetRef<Skeleton>       ──► .skeleton            ◄── independent of the mesh's!
  └── animation AssetRef<AnimSequence>   ──► .animseq ──(SkeletonId, Option)──► .skeleton

.blendspace ─► Skeleton? (Option<AssetRef>) + samples[].sequence Option<AssetRef<AnimSeq>>
.montage    ─► Skeleton? + segments[].sequence Option<AssetRef<AnimSeq>>
.animgraph  ─► Skeleton? + nodes{Sequence→AnimSeq | BlendSpace→BlendSpace | Output}
.physasset  ─► SkeletalMeshId + bone ids
.skeleton   ─► preview_mesh: SkeletalMeshId (editor-only hint)   ← a skeleton→mesh edge (cycle-prone with mesh→skeleton; benign because it is only a preview hint)
.animseq    ─► preview_mesh: SkeletalMeshId (same)
```

Gaps against the plan's target:

1. **No single Skeleton authority.** The component stores a `skeleton` ref *and* a `mesh` ref.
   `refresh_pose()` (`skeletal_mesh_component.rs:240-258`) only warns and **clears the palette**
   when the two disagree; and with `skeleton: None` there is no pose at all even though
   the mesh names its skeleton. Likewise the animation is applied only if its skeleton id equals the
   *component's* skeleton ref. Target: mesh decides; component `skeleton` becomes derived.
2. `.blendspace/.montage/.animgraph` hold `skeleton: Option<…>`, never validated against
   their sequences (no code compares a sample's `AnimSeq.skeleton` with the asset's).
3. Edges that don't exist yet: graph→montage, graph→IK rig/posedb, montage→skeleton is
   optional, blendspace→blendspace nesting is unchecked, and **no cycle detection** of any kind
   (nothing walks the graph).
4. `preview_mesh` makes `.skeleton`/`.animseq` point at `.skmesh`, the reverse of the
   mesh→skeleton edge. It is an editor hint, but the dependency walker must ignore it or it
   will report a cycle.

### 2.2 Project asset index capabilities (what "resolver" means here)

There are two separate things:

- **Project index** — `engine_fs::ProjectFileIndex` (`crates/core/engine_fs/src/file_index.rs`,
  SQLite). Table `asset_ids(asset_id, kind, path)`; API: `record_typed_asset_id`,
  `ensure_typed_asset_id` (also persisted to `.pulsar/asset_ids.json`),
  `path_for_asset_id_kind` (returns `None` if the id is **ambiguous**, i.e. >1 path),
  `paths_for_asset_id_kind`, `asset_ids_for_path[_kind]`, `clear_asset_ids`, move/remove
  tracking. It records **ids → paths only**: no dependency edges, no asset type metadata
  beyond `kind`, no diagnostics, no "used by". Ids are *registered lazily by editors on open*
  (`panel.rs:1859`, `animation_assets_editor.rs:147`, `library.rs:337`, `mesh_library.rs:240`,
  `asset_import/rig.rs:575`), not by a scan, so an asset never opened/imported is invisible to
  a component referencing it ("skeletal asset id is not indexed in this project" warning in
  `asset_id_for_ref`).
- **SceneDB asset ledger** — `pulsar_scenedb::assets`. `AssetId::declare(kind, source)` hashes
  *(kind, canonical path)* with FNV-1a, so SceneDB's runtime identity is **path-derived**;
  stable UUIDs are resolved to a path by Helio's `asset_id_for_ref` (which calls
  `engine_fs::open_project_file_index(root)` on **every** call, including from the handle-ledger
  collector), then `register_loader(kind, fn(&str)->payload)` + `register_binding::<T>(kind, fn)`
  load on a worker and bind into components. Only three kinds have loaders/bindings today:
  `skmesh`, `skeleton`, `animseq`. `.blendspace/.montage/.animgraph` have **no loader**.
- **Picker** — `ui_common::asset_picker::MeshAssetPicker` (`for_kind::<K>()`,
  `selected_asset_ref::<K>()`, `selected_or_create_asset_ref::<K>()`, `resolve_asset_ref_path`).
  It filters by kind/extension only: no skeleton-compatibility filter, no "show stable id",
  no navigate-to-tab, no repair UI for missing/ambiguous/stale refs.
- **Reverse deps ("Used By")** — nonexistent as a query. The only reverse scan is
  `skm_editor/src/core/hierarchy_references.rs::check`, a fail-closed walk of the *entire
  project manifest* (cap 200 000 files) that parses every `.animseq`, `.skmesh`, `.physasset`
  before skeleton bone edits. It ignores `.blendspace/.montage/.animgraph`, and is O(project)
  per edit.

### 2.3 What `.animretarget`, `.ikrig`, `.ikretarget`, `.posedb` need

For each: (a) a new `AssetKind` marker in `pulsar-reflection` `asset_refs.rs` with `KIND`
(`anim_retarget`, `ik_rig`, `ik_retarget`, `pose_db`) and `EXTENSIONS` (one each, no aliases);
(b) a `FileTypeDefinition` + editor id + `EDITOR_IDS` entry in `skm_editor/src/lib.rs`;
(c) a `skeletal_assets` codec module (versioned, extras map, `peek_id`) and id handling;
(d) index registration via `record_typed_asset_id` at load/save/import (same as existing);
(e) a SceneDB loader + binding in `helio-component` for runtime kinds (`ik_rig`, `ik_retarget`,
`anim_retarget`, `pose_db`, `animgraph`, `montage`, `blendspace`);
(f) a **dependency-edge query** (new, see decision D6): without it "dependency reporting",
"Used By", cycle detection and skeleton-compatibility diagnostics all degrade to the
O(project) manifest walk;
(g) `.posedb` additionally needs a derived-data location (recommend
`.pulsar/cache/posedb/<asset-id>-<hash>.bin`, never beside the source; `.pulsar/` is skipped by the project walkers already).

## 3. Runtime audit

### 3.1 How `SkeletalMeshComponent` ticks today

- Defined in `helio-component/src/components/skeletal_mesh_component.rs` (586 lines) plus
  `skeletal_notifies.rs` (1325 lines, ~700 of them tests).
- **Authored** (serialized): `mesh`, `skeleton`, `animation`, `playing`, `looping`,
  `playback_rate`. **Transient**: resolved `AssetId`s, `Arc` payloads, `elapsed_seconds`,
  `NotifyPlayback`, `bone_world_pose`, `physics_pose_active`, mesh GPU vectors,
  `bone_palette`, `gpu_bone_palettes`.
- **Tick**: `#[register_component_runtime] tick(&mut self, ctx, dt)` →
  `reconcile_notify_playback()` → `register_notify_events(hub)` → if
  `notify_playback.has_pending()` **return** → `advance(dt)` → `advance_notifies` →
  `refresh_pose()`.
- **Schedule position** (`pulsar_game/src/tick.rs`): AfterInput flush → Phase 1 ECS `schedule.run`
  (this includes physics, via `PhysicsBehavior`) → **Phase 1b `tick_live_components`**
  (the animation tick) → Phase 2 actors → AfterPhysics flush → Phase 3 scripts → AfterScripts
  flush → EndOfFrame → `pump_assets`. So animation runs **after** the physics step in the same
  frame and **before** script handlers; script-written parameters therefore take effect next
  frame (one-frame latency), and physics reads the *previous* frame's `bone_world_pose`.
  There is no explicit animation phase or declared ordering relationship; it is "whatever 1b does".
- **Pose evaluation**: `skeletal_assets::animation::evaluate_world_transforms` builds a
  `HashMap<String, Mat4>` keyed by cloned bone-id `String`s every tick, then
  `skinning::joint_matrices` maps through it, then `set_palette` clones `bone_palette` into the
  "previous" half and **rebuilds `gpu_bone_palettes` (current ++ previous) as a variable-length
  `#[gpu]` column** — all allocation-per-tick work proportional to bone count.
  This is the single biggest runtime performance risk (see §7); the plan's "no copies
  per entity, skeleton-indexed pose" requires replacing it with dense `Vec<Transform>` indexed
  by bone index.
- **Sequence selection**: only a single `AnimSeqAsset`; looping/playback rate global to the
  component; forward-only (`playback_rate.max(0.0)`; code comment: "the existing player
  supports forward playback only"). Blend spaces, montages, graphs, curves, root motion
  (`RootMotionSettings` is *authored in the asset* and evaluated by `root_motion.rs`, but
  **nothing in the component applies it**), additive, IK, retargeting: **not present**.
- **Notify cursor**: `NotifyPlayback { sequence, seconds (unwrapped f64), phase, generation,
  looping, pending: VecDeque, index: NotifyIndex }`. `NotifyIndex` (in `notifies.rs`) answers
  `collect(PlaybackInterval::{Advance, Seek, Paused})` with bounded output (1 000 000 cap),
  correct loop-seam arithmetic (`occurrence_time`), and a `generation` bump on seek/restart.
  Scrub/seek never publishes. Sequence-only; one cursor per component.
- **Delivery**: two channels. (1) the generic component-event bridge
  `events.animation_notify(AnimationNotifyEvent{sequence_id, notify_id, notify_name,
  clip_time, playback_time, cycle, generation})` through `ComponentEventWriter`
  (`SkeletalAnimationEvents` trait, `#[bp_event]`); (2) directed per-target events on
  `entity_channel(target)` under namespaced descriptor `animseq::<sequence-id>.<notify-id>`
  (fields entity/source/sequence_id/…/user fields), resolved from `NotifyTarget`
  (`Self_` or a class variable).
- ⚠ **Finding: `flush_animation_notifies` has no caller in this checkout.** Repo-wide search
  (all submodules, excluding `target/`) finds it only at its definition and the
  `pub use`. It is the only function that drains `notify_playback.pending` (via
  `publish_directed_notifies`). `tick()` returns early whenever `pending` is non-empty, and
  `advance_notifies` pushes crossings into `pending`. If nothing calls it, **the first notify
  crossing stalls that component's playback permanently**, and neither the generic nor the
  directed event is delivered. Unit tests in `skeletal_notifies.rs` call it directly so they
  pass. Same for `register_loaded_animation_notifies` (no caller; the tick registers lazily,
  so that one is benign). *(Unverified at runtime — the user runs the app; please check a
  sequence with a notify on a ticking component. Static search can miss dynamic wiring, but
  `pulsar_game::tick_once` has no hook.)* If confirmed this is a **pre-existing bug that
  Phase 3 depends on**; fix in the first runtime change (`R1` in §8).
- **GPU palette path**: per component `skin_vertices` (once-mirrored keyed by `geometry_key`),
  `gpu_bone_palettes` (`#[gpu(buffer="skeletal_bone_palette")]`, current ++ previous),
  `StaticMeshDraw` derived row via `GpuMirrorRegistration`; consumed by
  `engine_backend/src/scene/helio_bridge.rs` (`register_gpu_columns_growable(…, 128, …)`).
  Re-uploaded whenever the component row is mutated each tick.
- **Physics handoff and authority**: `physics_plugin/src/components/skeletal_rig.rs`
  (`PhysicsBehavior` `read`/`apply`/`after_steps`/`write_back`): `write_back` calls
  `component.apply_physics_pose(pose)` → sets `physics_pose_active = true` and writes
  `bone_world_pose` + palette; `clear_physics_pose()` on release. While active,
  `advance_notifies` returns early and `seek_animation` does not refresh the pose
  (**playback time and notify cursor freeze** while physics owns the pose — a defined but
  all-or-nothing authority; there is no per-chain/per-body blend). It requires
  `pose.len() == bones.len()`, so physics assumes the dense bone order of the *component's*
  skeleton. Authority is a single bool on the component; no ordering document exists.
  Tests: none in `skeletal_rig.rs` or `pulsar_physics::skeletal` (0 `#[test]`); the
  hand-off is covered only by `seeking_and_playback_preserve_an_active_physics_pose` in
  `skeletal_notifies.rs`.
- **Hot reload / teardown**: bindings re-run `refresh_pose` / `seek_animation(0.0)` when the
  asset payload changes (animation time resets to 0); removal handled by the component
  runtime registry; handle-ledger collection via `collect_skeletal_assets`. Not covered by a
  test that includes asset hot reload of a *playing* sequence.
- **Preview in the editor does not use the component.** The skm_editor viewport evaluates the
  same `evaluate_world_transforms` + CPU `skin_lod` directly (`viewport/scene.rs:212,861`);
  scrubbing is silent trivially because there is no event path. The plan requires "preview
  uses the real component runtime path": that is a **change**, not existing behavior.

### 3.2 What must change (by feature)

| Feature | Required change |
|---|---|
| Graph evaluation | Component holds `Option<AssetRef<AnimGraph>>`; compiled graph = immutable `Arc` in SceneDB payload; per-instance state struct (parameters, state-machine states, time cursors); replaces the single-sequence path while `animation` (direct sequence) is migrated on decode to a synthesized one-node graph or kept as a documented fast path |
| Pose data model | Dense bone-indexed `Pose { local: Vec<Transform>, curves, … }` buffers pooled per instance; remove `HashMap<String,Mat4>` and per-tick `clone`s |
| Schedule | Explicit animation phase (after input/script parameter writes, before physics read) or documented one-frame latency; deterministic fixed-order across components |
| Montage instances | Per-component list of instances `{asset, section, time, weight, blend state, id, generation}`, slot groups, command queue (play/stop/jump/set-next), completion events |
| Layering / masks | Per-bone mask tables derived from skeleton (cached per `(skeleton,mask)`); layered-blend evaluator; additive conversion |
| Root motion | Extract per layer into a `root_motion_delta`; apply through an owner-transform write at a defined point (today nothing consumes `root_motion.rs`) |
| IK / retarget / posedb | New evaluators operating on `Pose`; assets resolved via new SceneDB loaders; deterministic solver order |
| Notifies | Generalize `NotifyPlayback` to a per-*playback-instance* cursor (sequence, blend-space contributor, montage segment) with state begin/tick/end; keep `NotifyIndex` as the interval engine |
| Physics | Define the authority state (`Animation / Physics{bodies} / Blend`) and the single write point; physics `read` should take the *current* evaluated pose, which needs the animation phase to precede physics |
| Hot reload | Bindings must preserve playback time/state across payload replacement (today: reset to 0) |

## 4. Blueprint integration audit

- Discovery (`blueprint_editor/src/features/events/engine_events.rs`): `animation_catalog` calls
  `engine_backend::project_animation_notify_events(root)`, which is a **recursive directory walk
  parsing every `.animseq`** (depth ≤16, skips hidden/`target`/…) — not the index, not cached.
  Results are filtered to sequences referenced by the *class prefab's enabled
  `SkeletalMeshComponent.data.animation`* (`referenced_animation_asset_ids[_from_json]`,
  `animation_asset_ids_from_prefab`). **Consequences:** only the single direct `animation` ref
  counts; notifies inside blend-space samples, graph nodes or montages would never appear;
  asset-id filter requires the prefab JSON field name `animation` to stay (or the function must
  walk the dependency closure through the new index).
- Palette nodes: `animation_notify_component_nodes` builds
  `event::on_component::SkeletalMeshComponent.<descriptor name>` nodes with a `component_ref`
  input (component-scoped, not global), category `Events/Components/SkeletalMeshComponent/Animation/<seq8>`.
  Fields come from the descriptor, so notify user fields are typed pins already.
- Compiler (`blueprint_editor/compiler/src/lib.rs:2425` `component_event_name`): strips the
  `SkeletalMeshComponent.` prefix when the rest starts with `animseq::`, keeping
  `animseq::<seq>.<notify>` as the canonical event name; test
  `animation_notify_entry_point_requires_component_scope` and a footstep compile test
  (`compile.rs:1988-2013`) cover it. Legacy `event:on_component…` spellings are normalized.
- There is **no Blueprint-callable animation command surface**: `seek_animation`,
  `restart_animation`, `stop_animation` are Rust `pub fn`s without a script binding
  (nothing in `plugins`/`crates` calls them outside tests). Blueprint can only set the
  authored properties (`playing`, `looping`, `playback_rate`, `mesh`, `skeleton`,
  `animation`) through the property projection, whose `skeletal_mesh_property_written`
  hook re-attaches assets.
- Needed: (1) typed graph parameters (`set_param_float/bool/int/vector`, validated against the
  graph's declared parameter list; names stable, types fixed); (2) Montage commands
  (`play`, `stop`, `pause`, `jump_to_section`, `set_next_section`, `set_rate`) returning an
  instance handle usable in completion events; (3) notify *state* events with
  begin/tick/end and a `phase` field; (4) event descriptors keyed `montage::…`,
  `animgraph::…` namespaces with source `AssetRef`, instance id and cycle; (5) discovery via
  the dependency closure from the component's mesh/graph refs, from the index, with a
  cached catalog invalidated by asset save events (not per-open scans); (6) component ref
  resolution unchanged (already per-component, not global).
- The Blueprint side of notify discovery is in a **separate repo** (blueprint_editor) while
  descriptors are produced in Helio; the `engine_backend` re-export is the seam
  (`engine_backend/src/lib.rs:22`).

## 5. Editor audit

### 5.1 Existing

`skm_editor/src` (≈27 k lines including `skeletal_assets`): one `SkeletalAnimEditorPanel`
(`editor/panel.rs`, 1864 lines) serving three `EditorMode`s — `Skeleton`, `Sequence`, `Mesh`.

- **Timeline**: custom GPU-drawn (`rendering/timeline/{layout,panel,renderer,gutter,toolbar}.rs`):
  ruler, summary, a pinned **notify lane** (`NOTIFY_HEIGHT`, enlarged hit target, `Hit::Notify`,
  `Hit::NotifyRow`, one hit-test unit test), key/selection/box tools, playhead, zoom.
  Notify lane has no stacking/collision offsets, no hover/disabled/state-range visuals, and
  **notifies are instantaneous** (no ranges).
- **Viewport**: `rendering/viewport/{scene,renderer,camera,panel}.rs`: CPU pose + CPU skinning,
  bone overlay, motion-path ghost samples, camera orbit. Not the Helio runtime path.
- **Notify editing**: `add_notify/select_notify/set_notify_name/set_notify_time/delete_notify`
  on the panel (`panel.rs:646-726`), edited via `properties_panel.rs`; they set `is_dirty`
  but the only `SharedHistory` in the panel is `hierarchy_history` (bone edits) — **notify
  edits are not undoable** *(by reading; unverified at runtime)*. Fields/targets round-trip through the
  codec (`NotifyField`, `NotifyTarget`).
- **Undo/dirty/save**: `plugin_editor_api::UndoService` + `SharedHistory` for hierarchy edits;
  `is_dirty`/`tab_unsaved`; atomic save. There is **no document-revision/conflict mechanism**
  across tabs: two tabs on the same file save last-writer-wins.
- **`animation_assets_editor.rs` (449 lines)**: one `AnimationAssetEditor` with `Kind::{BlendSpace,
  Montage, Graph}`; holds the asset as a **`serde_json::Value`** and edits it through
  `get_json_path/set_json_path`; UI = title, a skeleton ref picker, one picker per entry, and
  "Add … " buttons (add-only: no delete, reorder, x/y/start/rate editing, validation,
  visualization, undo; saves immediately and logs errors). It is the "generic form" the plan
  forbids. It is a **delete** (replace by three purpose-built editors); only its
  open-time id-registration logic is worth carrying over (into a shared helper).

### 5.2 What the Graphy animation profile needs

Graphy (`6c3da62`) provides: `GraphDescription {metadata, nodes: HashMap<id,NodeInstance>,
connections, comments}`, `NodeInstance {node_type, position, inputs, outputs, properties,
typed_properties}`, `DataType::{Exec, Data(TypeInfo)}`; `graphy-gpui`'s `GraphProfile`
(`id, presenter, type_resolver, can_connect, catalog, drag_match, create_node, create_reroute,
refresh_pins, pin_commands, is_deletable, macro_target, menu_items, run_menu_item, decorate`),
optional `GpuiProfile` (custom host rows); `Presenter` (`node, pin, pin_label, pin_widget,
row_height, wire, sections, comment`); `GraphCanvas` with `CanvasEvent::Edited`, `GraphCommand`
batches, `ChangeSet::undo`, `diff_commands`. **There is no animation profile, no animation node
catalogue and no animation sample scene in Graphy.** (`graphy-gpui/src/profile.rs:2` and
`render_scenes.rs:8` mention "animation" aspirationally; `render_scenes` is meant to be compared
with the mockup.) The plan says "migrate to the completed Graphy graph document and animation
profile"; the profile does not exist and must be written — recommended in the
`skeletal_animation_plugin` crate (domain code in the domain plugin, like `RenderProfile` /
`ShaderProfile`), not in Graphy.

Needed for the profile:

- **Pin/type universe**: `Pose` (local), `PoseAdditive`, `Float/Bool/Int/Vector`, `Curve`,
  `Mask`, `SlotName`, `Time/SyncGroup`, asset-ref pins. Map to `TypeInfo` strings with a custom
  `TypeResolver` (the shader editor already does this chained behind `StructuralResolver`).
  Pose flows as **data wires, not exec wires** (mockup `.bp-wire path.pose` uses purple).
- **Node catalogue**: Sequence Player, Blend Space Player, Blend (2/N), Layered Blend per Bone,
  Apply Additive, Cached Pose (save/use), Slot, State Machine, Two-Bone IK, FABRIK, Retarget,
  Pose Search, Parameter Get, Output Pose, sub-graph call.
- **State machine & transition views via existing hooks**: a State Machine node uses
  `macro_target` to open a *state graph* (a second `GraphDescription` in the asset, as
  `.rendergraph` does for sections); states are nodes in that graph; transitions are
  connections that carry properties (`duration`, `curve`, `priority`) — but Graphy
  connections **have no property bag** (to verify in `Connection`), so a transition is either a
  dedicated "Transition" node inserted on the wire (works with today's API, recommended) or
  needs a Graphy extension. The transition's rule graph is a third nested graph via
  `macro_target`. Selection → inspector comes from the host (as in Blueprint/Shader).
- **Missing Graphy extension points** (candidate small Graphy PRs, none blocking the first
  slice): (1) wire/connection metadata and per-wire click target (`Hit::Wire` exists? needs
  checking) for transition selection; (2) `Presenter` shape for state nodes (rounded/
  pill) and "active state" runtime highlight via `decorate` — exists; (3) breadcrumb /
  nested-graph navigation helper (macro nav exists in Blueprint code, partly in `GraphProfile`);
  (4) per-node live pose/weight badges (decorate is enough); (5) multiple-document
  `GraphCanvas` per asset (render_bps pattern, exists).
- **Compiler**: Graphy's compile pipeline (`graphy` crate, data-flow/exec-flow, code generation)
  is Blueprint/shader oriented. For animation we need only **validation + lowering to an
  evaluation plan** (topological order of pose nodes); recommend a new animation-owned
  compiler (plan §Scope says so) that consumes `GraphDescription` directly, reusing Graphy's
  analysis (topological sort, diagnostics type) where it fits.

### 5.3 Gaps vs each mockup view

Mockup = single HTML file with five views (tabs): `blueprint`, `graph`, `sequence`, `blend`,
`montage`. It is a layout/interaction spec; its graph view uses ad-hoc `.bp-node` boxes, not
Graphy visuals. It has **no** Retarget, IK Rig, IK Retargeter or Pose Search views.

| View | What exists natively | Gap |
|---|---|---|
| Blueprint | Blueprint editor (separate repo), component details via reflection | typed Mesh/Graph refs on the component, graph param/montage nodes, notify-state events (§4); "resolved Skeleton from mesh" display |
| Animation Graph | generic form (§5.1) | whole view: Graphy profile, state-machine/transition views, preview panel (pose/state/weights/curves/slots/root motion/trace), parameter panel, tree, inspector |
| Animation Sequence | `Sequence` mode: timeline + viewport + hierarchy + properties | notify states/ranges, lane collision stacking, hover/disabled visuals, payload editor completeness, undo for notify edits, curves & root-motion editing UI *(root motion/curves exist in the asset; no panel UI found by grep for the editor beyond the data model — to verify)*, real-runtime preview, preview-instance selector |
| Blend Space | nothing visual | 1D strip/2D plot, triangulation, draggable samples, live weights, validation |
| Montage | nothing visual | multi-track timeline (segments/slots/sections/notify lanes), section links, layered preview |
| Skeleton | hierarchy editor + viewport + sockets | derived "Used By" panel (index), canonical-tab navigation |
| Skeletal Mesh | `Mesh` mode | skeleton compatibility/authority UI, LOD list (asset has LODs; confirm editor exposes) |
| Retarget / IK Rig / IK Retargeter / Pose Search | none | entire editors + mockup states |

## 6. Test / fixture inventory and migration map

### 6.1 Existing tests and fixtures

| Area | Location | Count / kind | Notes |
|---|---|---|---|
| Asset codecs | `skeletal_assets/src/{assets,skmesh,skeleton,skinning,sockets,math,ids}.rs` | 9 + 11 + 14 + 6 + 3 + 6 + 1 unit tests | skeleton/animseq/skmesh round-trip, future-version refusal, sockets |
| Notifies/curves/root motion | `skeletal_assets/tests/animation_features.rs` | 24 tests, 884 lines | notify crossings, curves, root motion, version migration, 1 M crossing bound |
| Component runtime | `helio-component/.../skeletal_notifies.rs` | ~17 tests | cursor, dispatch order, catalog, independence of shared-sequence instances, physics pose; `helio_component` GPU tests excluded from CI nextest (`.config/nextest.toml`) |
| Editor | `skm_editor/src/**` | hierarchy_tests (14), library/mesh_library (8/6), validate (5), timeline layout (9), viewport render/real_sample (3+1; skipped w/o env) | `gpui` test-support for panel; naga WGSL validation |
| Importer | `asset_import/tests/{rigged_import,real_sample,multi_mesh,companion}.rs`, `helio-asset-compat/src/rig_converter.rs` (3) | 7 rigged tests; `real_sample` needs env `SKM_SAMPLE_DIR/BrainStem.glb` | the only real-data fixture is an **external** BrainStem; none is committed |
| Blueprint | `blueprint_editor/compiler/tests/compile.rs` (footstep notify) , `engine_events.rs` tests | a few | scope check + canonical name |
| Physics | `physics_plugin`, `pulsar_physics` | 0 skeletal-specific tests | hand-off only covered from the animation side |
| Graphy | `graphy/tests/*`, `graphy-gpui/tests/interaction.rs` | many generic | none animation-specific |

**Fixtures:** the repository contains **zero** committed `.skeleton/.skmesh/.animseq/.blendspace/.montage/.animgraph`
files (and none in `assets/examples`). "Real fixtures" for migration tests must be created
and committed (small hand-written v1 JSON for blendspace/montage/animgraph; a tiny
generated rig for the rest; a v1/v2/v3 `.animseq`; a `.skmesh` v1 binary) — recommended under
`plugins/vendor/skm_editor/skeletal_assets/tests/fixtures/` with a generator test that
asserts the checked-in bytes still parse (so the fixture can't rot).

### 6.2 Migration map and submodule order

| Change | Repo / path | Action |
|---|---|---|
| New asset kinds, drop alias extensions | `pulsar-reflection` `asset_refs.rs` | extend |
| Codecs v2 (blendspace/montage/animgraph/animseq), 4 new asset codecs, ids unification, dependency walker | `skm_editor/skeletal_assets` | extend (+ new modules) |
| Registrations, editors, profile, preview | `skm_editor/src` | extend; delete `animation_assets_editor.rs` (replace); `lib.rs` test table extended |
| Component authoring + runtime + loaders + notifies | `helio-component/.../skeletal_*` | extend/restructure (split into `animation/` module: pose, graph runtime, montage, notify, root motion) |
| Importer: keep; adds `ImportOutput` kinds only if retargeting auto-creates assets | `asset_import/src/rig.rs`, `helio-asset-compat/rig_converter.rs` | keep (`rig_converter` also needs to preserve what posedb/ik need — bone names; no change expected) |
| Physics hand-off | `physics_plugin/.../skeletal_rig.rs` | extend (authority state) |
| Blueprint nodes + discovery + compiler | `blueprint_editor` `engine_events.rs`, `compiler/src/lib.rs` | extend |
| Graphy extension points (only if §5.2 gaps confirmed) | `graphy`, `graphy-gpui` | extend (small) |
| Host registration / docs / pins | `ui_core/builtin_editors.rs` (no change), `.agents/*.md`, `Cargo.toml` workspace members, `.github/submodule-pin-exceptions.txt` | docs: add `.agents/ANIMATION.md`, update `CRATES.md`, `REFLECTION.md`, `SCENEDB_MIGRATION.md` |

**Merge order** (dependency-first per `.agents/CRATES.md` "Submodule pins"):
`pulsar-reflection` → `skm_editor` (skeletal_assets, editors) → `helio` (helio-component) →
`physics_plugin`, `blueprint_editor`, `graphy*` (independent of each other) → Pulsar-Native
(re-pin each to its upstream main commit, then merge). Each submodule PR pins its dependency
to a feature branch while in review (allowed; list in `.github/submodule-pin-exceptions.txt`).
Watch: Helio's `Cargo.toml` references `pulsar_reflection` by a git rev (`091eae5…`) while the
root workspace patches it to the path; `helio-component` takes `skeletal_assets.workspace =
true` but Helio's own manifest has no `skeletal_assets` entry — Helio only builds inside the
Pulsar workspace *(not verified by building)*. Plan every cross-repo type change with that in mind.

### 6.3 Format migration risks

1. Silent field loss on resave (no extras maps) — every v2 codec needs `extra` + a test.
2. Random-id-on-missing-id for the three high-level assets (see §1.2).
3. `.animseq` writes the version lazily (1/2/3); adding v4 must keep "no features ⇒ keep old
   version" so unchanged files do not churn; older editors refuse v4 (acceptable, "newer"
   error exists).
4. `.animgraph` v1→v2: node ids are strings like `sequence_<uuid>`/`output`; Graphy ids are
   strings too, so ids can be kept; connections lack pin names (`from`/`to` are node ids) →
   mapping to `out`/`in` pins needs explicit convention + tests; `Output` becomes the
   protected output-pose node.
5. Component: `skeleton`/`animation` fields in saved scenes and prefabs. Prefab JSON is read
   directly by Blueprint (`data.animation`); removing/renaming it breaks event discovery
   (§4) unless migrated in step.
6. SceneDB `AssetId` is path-derived: moving a file changes the runtime AssetId but the
   UUID→path resolve in Helio adapts; hot-reload by id→path change is untested.
7. `.skmesh` skeleton ref is `Option` and "kept as is even when no skeleton exists": making the
   mesh the authority needs a decision for meshes with `None` (import always sets it; manual
   creation is not offered, so likely none in practice).
8. `.skelanim` legacy conversion writes `.skeleton`+`.animseq`; keep it working while codecs change.

### 6.4 Risks list (summary)

1. `flush_animation_notifies` unwired → notify stall (§3.1) — correctness blocker for Phase 3.
2. Per-tick `HashMap<String,…>` + Vec clones per component: will dominate cost at 200/500
   bones and many components.
3. No dependency-edge store: Used By, cycles, compatibility and Blueprint discovery all depend on a
   new capability (D6); building it as an O(project) walk would reintroduce the cost the manifest-walk
   in `hierarchy_references.rs` already has.
4. Ids are registered only when an editor opens a file → unindexed assets can't resolve.
5. Graph connections have no property bag; transitions need a design choice (D5).
6. Physics reads one-frame-old pose; any change to tick position changes physics behavior.
7. Preview divergence: editor viewport uses a separate CPU evaluator; if the runtime
   gains layering and the preview does not, authoring lies about runtime results.
8. Four-repo type ripple through `skeletal_assets` (§1.3).
9. No committed fixtures; migration tests will have to author them first.
10. No notify-edit undo and no cross-tab revision control; the plan's "one undo/save contract"
    is new infrastructure.
11. Plan references (`POLISH.md`, "completed Graphy animation profile") don't match the checkout (§0, §5.2).

## 7. Baseline plan (not executed)

### 7.1 What exists

- `profiling::profile_scope!` markers (used in `pulsar_game::tick`) and the `pulsar-profiling`
  checkout; no animation-specific scopes.
- `content_dedup_benchmarks.rs` (helio-component) as a pattern for an integration-test style
  benchmark; `docs/scripting-baseline.md` / `script-vm-benchmarks.md` show the repo's
  documented-baseline convention.
- Importers/fixtures: `BrainStem.glb` via `SKM_SAMPLE_DIR` (external; bone count not
  measured), `rig_converter` can produce rigs from glTF.
- **No** benchmark of skeletal evaluation, palette upload, notify collection, asset
  resolution, timeline scrubbing or graph evaluation exists. `criterion` is not used in the
  affected crates.

### 7.2 Harness to build (before any optimization)

A single bench target `skeletal_assets/benches/` (CPU/pure) plus one `helio-component`
integration binary (component + SceneDB, GPU upload measured separately), all emitting
a machine-readable JSON (`target/anim-baseline/<run-id>.json`) with machine header (CPU, GPU,
driver, OS, rustc, `cargo --profile`, features, git SHAs of all six repos). Workloads:

| ID | Workload | Measures |
|---|---|---|
| W1 | Synthetic rigs 68 / 200 / 500 bones (generated chain+tree, deterministic seed), 4–6 s clip at 30 fps, linear and cubic keys | single-sequence eval ms (p50/p95), allocations/tick, bytes/tick |
| W2 | 1, 100, 1 000 components sharing one clip, independent phases | tick wall time, scaling, memory (RSS delta + payload sharing check) |
| W3 | Clip length 1 s / 10 s / 60 s | sampling cost vs. key count |
| W4 | Dense notifies: 10 / 100 / 1 000 per clip, high delta (loop wrap multiples) | `NotifyIndex::collect` p50/p95, dispatch latency through hub |
| W5 | 1D (5 samples) and 2D (9, 25 samples) blend spaces | weight solve + blend eval — *requires the feature; baseline = N/A now, record "does not exist"* |
| W6 | Layered montages 1/2/4 concurrent on 68-bone rig | same, *N/A today* |
| W7 | Asset resolve: cold/warm `asset_id_for_ref` for N refs; index lookup; catalog build (`project_animation_notify_events`) over 100/1 000/10 000 asset projects | ms, file reads |
| W8 | Root-motion extraction + physics hand-off (`apply_physics_pose`, 68 bones) | ms, copies |
| W9 | GPU palette upload for N skinned meshes (needs GPU adapter; run by user) | upload bytes/frame, ms |
| W10 | Editor timeline scrubbing: layout+hit-test+draw of 68-track × 300-key clip at several zooms | frame ms (needs gpui test-support or a bench with the layout code only) |

Methodology: release profile (same `[profile]` as `just release`), 30 warm-up + 300 samples,
fixed seed, pinned thread count, single machine, same commit pair before/after, report p50/p95/max,
allocation counts via a counting global allocator in the bench binary (no external tool),
no LOC-based claims.

**Built by this phase:** none. **To build first (phase B0):** W1–W4, W7, W8 (pure-CPU, no
GPU). **User must run:** W9 and W10 on their GPU machine, plus all of the above on their
reference machine to produce *the* baseline; I did not run any.

## 8. Proposed phase breakdown

Repository-sized changes, each independently reviewable and mergeable in dependency order.
IDs are suggestions; "needs" = hard prerequisite.

| ID | Change | Repo(s) | Needs | Exit criterion |
|---|---|---|---|---|
| B0 | Benchmark harness (§7) + commit fixtures | skm_editor, helio | — | baseline JSON producible by one command (user runs it) |
| R0 | Fix notify delivery wiring (`flush_animation_notifies` called from the engine tick after scripts release World, before `AfterScripts` flush) + a tick-level regression test through `pulsar_game::tick_once` | helio, pulsar_game/Pulsar-Native | — | a ticking component with a notify keeps advancing and both events arrive |
| A1 | Reflection: add 4 kinds, drop alias extensions; typed ids | pulsar-reflection | — | `cargo test -p pulsar_reflection` |
| A2 | `skeletal_assets` v2 codecs: blendspace, montage, animgraph(Graphy doc), animseq v4 (notify states), extras maps, migration + fixtures/tests; new codecs for animretarget/ikrig/ikretarget/posedb (data + validate only, no editors) | skm_editor | A1 | round-trip/old-version/future-version/duplicate-id/cycle tests |
| A3 | Dependency-edge capability (extractors per kind) + cycle/skeleton-compat validation; Used-By query; replace `hierarchy_references` walk | engine_fs (Pulsar-Native) + skm_editor | A2, D6 | tests incl. 10 000-asset project |
| R1 | Component: mesh-derived skeleton authority, dense `Pose`, loaders for new kinds, remove per-tick HashMaps; compat for direct `animation` ref | helio | A2, R0 | existing 17 notify tests + new pose tests; B0 shows no regression |
| R2 | Graph compiler/evaluator (sequence, blendspace, blend, state machine, params, cached pose, additive/layered) | skm_editor (compiler) + helio (runtime) | R1, A2 | deterministic eval tests; two components independent |
| R3 | Montage runtime + slots + commands + completion | helio | R2 | layered base+upper-body test |
| R4 | Notify generalization (states, instances, montage/blendspace sources), directed delivery | helio (+ blueprint_editor catalog) | R3 | begin/tick/end exactly-once matrix |
| R5 | Root motion + physics authority | helio, physics_plugin | R3 | defined handoff tests |
| R6 | IK (2-bone, FABRIK) + `.ikrig` runtime | helio, skm_editor | R2 | solver tests |
| R7 | Retarget (`.animretarget`, `.ikretarget`) | helio, skm_editor | R6 | chain-mapping validation tests |
| R8 | Pose Search (`.posedb` build + query) — *optional per D3* | helio, skm_editor | R3 | deterministic build/hash tests |
| E1 | Shared editor infra: selection/inspector contract, document revision, notify/undo, shared ref picker with skeleton filter | skm_editor, ui_common | A3 | undo for notify edits; stale-save conflict test |
| E2 | Sequence editor upgrades (notify states, stacking), runtime-path preview | skm_editor | R4, E1 | native screenshot review |
| E3 | Blend Space editor | skm_editor | R2, E1 | |
| E4 | Montage editor | skm_editor | R3, E1 | |
| E5 | Animation Graph editor (Graphy profile + state machine/transition views) | skm_editor (+ graphy PR if D5 chooses extension) | R2, E1 | |
| E6 | Retarget / IK Rig / IK Retargeter / Pose Search editors | skm_editor | R6–R8, E1 | |
| BP1 | Blueprint: parameter/montage nodes, state events, closure-based discovery | blueprint_editor | R3, R4, A3 | compile + palette tests |
| M1 | Mockup: `.montage`, 4 new views, end-to-end flow | Pulsar-Native `docs/mockups` | — (can go early) | |
| V1 | Docs (`.agents/ANIMATION.md`, CRATES/REFLECTION/SCENEDB), pins, CI, final before/after tables | Pulsar-Native | all | |

Exact commands (workspace root `D:\GitHub\Pulsar-Native`; GPU-dependent tests on the user's machine):

```
just check
cargo test -p skeletal_assets
cargo test -p skeletal_animation_plugin
cargo test -p helio_component                # GPU binaries need an adapter; CI excludes some (see .config/nextest.toml)
cargo test -p asset_import                   # real_sample needs SKM_SAMPLE_DIR=<dir with BrainStem.glb>
cargo test -p pulsar_physics
cargo test -p physics_plugin
cargo test -p pulsar_game
cargo test -p blueprint_compiler             # package name: verify in plugins/vendor/blueprint_editor/compiler/Cargo.toml
cargo test -p pulsar_reflection
cargo test -p engine_fs
(cd crates/third-party/graphy; cargo test --workspace)
(cd crates/third-party/graphy/crates/graphy-gpui; cargo test; cargo check --features gpui)
just clippy
just test
just check-submodule-pins
```

(The `blueprint_compiler` package name and per-package names above come from directory
names / `Cargo.toml` files I read; I did not run any command, so verify before putting them in CI.)

## 9. Open decisions for the user

| # | Decision | Options | Recommendation |
|---|---|---|---|
| D1 | Canonical Montage extension | `.montage` / `.animmontage` | **`.montage`** — already registered, saved, in Reflection; only the mockup differs. Drop the `anim_montage`/`anim_graph` extension aliases; keep `KIND` strings |
| D2 | Skeleton authority | mesh only / mesh + component override | **Mesh only.** Component keeps no authored skeleton; on decode an old `skeleton` value is checked against the mesh and a mismatch becomes a repairable diagnostic (not silently dropped). No override unless something concrete needs it (none found) |
| D3 | First delivery scope | include Pose Search + IK Retargeter now, or ship Graph/Blend/Montage/Notify/IK Rig + Retarget Profile first | **Defer `.posedb` (R8/E6-part) and the IK Retargeter** to a second delivery; deliver A/R0–R5, IK Rig solvers, Retarget Profile, all three authoring editors and Blueprint. The plan forbids stubs, so partial inclusion must be all-or-nothing per asset type |
| D4 | Where the animation Graphy profile lives | `skm_editor` / Graphy repo | **`skm_editor`** (domain in domain plugin); Graphy only gets generic extension points |
| D5 | Transition representation | Transition node on a wire / connection property bag in Graphy | **Transition nodes** (no Graphy change, still selectable/inspectable); revisit if visuals demand edge-labels |
| D6 | Dependency/“Used By” store | extend `ProjectFileIndex` with an `asset_deps` table fed by per-kind extractors / in-memory scan cache / keep manifest walk | **Extend `ProjectFileIndex`** (engine_fs) with deps table, populated on save/import/refresh_path; it already owns id→path and move tracking |
| D7 | Animation tick position | keep Phase 1b after physics (1-frame latency) / new explicit phase before physics | **New explicit animation phase before physics read**, with script-written params applied the next frame (documented), because IK/physics blending needs the current pose |
| D8 | Notify states in `.animseq` | add `duration` to `AnimationNotify` (v4) / separate `notify_states` list | **Separate `notify_states` list** (begin/end semantics differ, instant notifies stay byte-stable for v3 readers) |
| D9 | Direct `animation` ref on component after graphs land | keep as permanent fast path / migrate to synthesized one-node graph | **Migrate on decode** to an inline single-sequence player internal to the component state (no `.animgraph` file created), remove the second authoring field after one release |
| D10 | Preview source of truth | keep editor CPU evaluator / preview via transient runtime component | **Runtime component in a preview world** (the plan's requirement); keep the CPU evaluator only for static skeleton/mesh bind-pose views |
| D11 | Fixtures | commit generated small fixtures / rely on external BrainStem | **Commit small generated fixtures**; keep BrainStem as optional extra |
| D12 | Fix R0 (notify wiring) first, on its own | yes / fold into R1 | **Yes, on its own**, ideally after you confirm the stall by running a notify scene |

## 10. Discrepancies between the plan and the checkout

1. No "completed Graphy animation profile" exists; only the generic profile API (§5.2).
2. `POLISH.md` is not at the repo root; the skm_editor one is empty (§0).
3. Plan: "SceneDB asset index/resolver" — the resolver is Helio's use of the SQLite
   `ProjectFileIndex` plus SceneDB's path-hashed `AssetId` ledger; there is no SceneDB-side
   stable-UUID index (§2.2).
4. Plan says `SkeletalMeshComponent` "publishes animation notifications"; true in tests, but
   the engine tick never drains them (§3.1).
5. Plan: "RootMotion … evaluated"; the asset-side evaluator exists, the component never uses it.
6. Plan: "Blueprint ... already discover/compile sequence notify events" — true for a *direct*
   sequence ref only, via a full project directory walk (§4).
7. Plan: `.skmesh` "typed Skeleton ref"; actually a raw `SkeletonId`, and the component can
   hold a contradicting ref (§2.1).
8. Plan: `.montage`/`.animgraph` Reflection kinds; their `EXTENSIONS` also list non-file
   spellings (§1.1).
9. The `lib.rs` registration test covers 4 of the 7 file types, not blend/montage/graph.
