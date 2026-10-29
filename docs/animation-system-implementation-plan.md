# Animation System Implementation Plan

## Claude Code implementation prompt

You are working in `D:\GitHub\Pulsar-Native`, a Rust game engine whose editor
is also the runtime. Read `AGENTS.md` and the relevant `.agents/` architecture
notes before changing code. Graphy is the shared graph platform and its
migration is a prerequisite assumed complete for this task. Consume Graphy's
public document, compiler, reflection/type adapter and GPUI/WGPU canvas APIs;
do not create an animation-only graph model, renderer, input stack or editor
shell.

Implement one coherent, production-grade animation authoring and runtime
system, including its asset formats, SceneDB references, runtime evaluation,
Blueprint integration, editors, migration, tests and performance evidence.
Treat `docs/mockups/animation-editor.html` as the UI shell and visual/workflow
reference for Blueprint, Animation Graph, Animation Sequence, Blend Space and
Animation Montage. It is a demo/reference, not the implementation and not a
source of serialized fixture values. Extend the mockup with separate design
states for any new asset editors introduced below before or alongside their
implementation.

Do not leave generic JSON forms, placeholder toast actions, empty crates,
unreachable editors, fake preview state or permanent forwarding wrappers as
the final result. Preserve user work, branches and submodule state. Follow
`.agents/CRATES.md` for dependency-first submodule changes. Do not force-push,
reset, merge, publish or close issues. Keep each repository change reviewable
and report the exact tests, measured performance, remaining limitations and
dependency pins.

## Starting point verified in this checkout

The animation implementation already has valuable foundations; extend and
consolidate them rather than replacing them blindly:

- `plugins/vendor/skm_editor/skeletal_assets/` owns skeleton, skeletal mesh,
  animation sequence, curves, root motion, sockets, notify data and the current
  Blend Space/Montage/Animation Graph schemas. The current files include
  `.skeleton`, `.skmesh`, `.animseq`, `.blendspace`, `.montage` and `.animgraph`.
- `plugins/vendor/skm_editor/src/editor/panel.rs`, `src/rendering/` and
  `src/ui_components/` contain the skeleton/mesh/sequence editor, a custom GPU
  timeline and a skinned viewport with a real animation preview. Preserve and
  build on these renderers where they fit; do not replace them with a web view.
- `plugins/vendor/skm_editor/src/animation_assets_editor.rs` currently hosts
  Blend Space, Montage and Animation Graph modes. Their data types are much
  less capable than the target system, and the UI must become distinct,
  purpose-built asset editors rather than generic forms.
- The current `.animgraph` schema in
  `skeletal_assets/src/animation_sets.rs` only models Sequence, Blend Space
  and Output nodes with string node/edge IDs. Migrate it to the completed
  Graphy graph document and animation profile. Preserve stable asset identity
  and provide a tested versioned asset migration.
- `crates/renderer/helio/crates/helio-component/src/components/`
  contains `skeletal_mesh_component.rs` and `skeletal_notifies.rs`. The
  SceneDB `SkeletalMeshComponent` already resolves mesh, skeleton and sequence
  refs, owns per-instance playback state, drives skinning data and publishes
  animation notifications. Keep it as the runtime wrapper and lifecycle/tick
  owner; evolve its authored refs and transient instance state rather than
  moving runtime ownership into an asset or editor plugin.
- `plugins/vendor/blueprint_editor/src/features/events/engine_events.rs` and
  `plugins/vendor/blueprint_editor/compiler/` already discover/compile
  sequence notify events by asset identity and component context. Extend this
  path for notify states and Montage/graph event sources without introducing
  global events or storing live entity handles in assets.
- `plugins/vendor/physics_plugin/src/components/skeletal_rig.rs` consumes the
  component's evaluated pose for articulated physics. Preserve a clear
  animation-to-physics handoff and define authority when physics drives bones.
- `crates/core/asset_import/src/rig.rs`,
  `crates/renderer/helio/crates/helio-asset-compat/src/rig_converter.rs`,
  `crates/editor/ui_core/src/builtin_editors.rs` and the plugin registration
  in `skm_editor/src/lib.rs` participate in import, compatibility and asset
  editor registration. Update all of them as needed.
- Pulsar Reflection already defines typed `AssetRef` markers for skeleton,
  skeletal mesh, Sequence, Blend Space, Montage and Animation Graph in
  `crates/external/pulsar-reflection/.../asset_refs.rs`. Reuse the shared
  SceneDB/project asset index and typed-reference picker mechanisms. Stable
  IDs are identity; paths are resolved locations/cache data.

Verify these findings against the live checkout and submodule commits before
implementation. In particular, identify the active Graphy package/API,
SceneDB asset resolver, component lifecycle hooks, importer versions, existing
animation tests and the real file extensions. Record the status of the
Reflection and Helio submodules and preserve unrelated local changes.

## Goals and non-goals

### Goals

- Make animation assets, animation graph evaluation and per-component runtime
  playback form one end-to-end system.
- Support layered character animation: locomotion/base graph, state machines,
  Blend Spaces, sequences, additive poses, named slots, concurrent Montage
  instances, per-bone masks, curves, root motion, notify events/states, IK and
  motion matching.
- Make every authored link a typed, stable SceneDB `AssetRef`; keep reverse
  “Used By” lists as derived queries, never serialized circular references.
- Make `SkeletalMeshComponent` the runtime wrapper and the per-entity owner of
  evaluation state, tick integration, event delivery, physics pose handoff and
  renderer-facing bone palettes.
- Make each asset editor visually distinct and genuinely interactive, while
  using the existing Pulsar editor chrome, shared property inspector, shared
  asset pickers and Graphy canvas.
- Preserve existing `.skeleton`, `.skmesh` and `.animseq` assets and migrate
  current high-level assets without losing IDs, features or unknown supported
  fields.
- Measure evaluation, blending, notify, asset resolution, timeline and graph
  performance on representative rigs before claiming benefits.

### Scope boundaries

- Graphy owns generic graph documents, graph compilation infrastructure,
  rendering, selection, graph editing commands and type/reflection adapters.
  Animation owns pose values, animation node catalogue, graph semantics,
  validation and runtime evaluation.
- Skeletal animation plugin owns animation asset codecs, asset-specific
  authoring UI, skeleton/mesh/sequence preview tooling and animation editor
  lifecycle. Helio component owns runtime playback/evaluation integration and
  renderer/physics handoff. Blueprint owns gameplay graph nodes, component
  binding and event filtering. SceneDB owns entity/component and asset-ref
  identity/resolution semantics.
- Keep imported skeletal mesh geometry and source animation extraction in the
  existing import path. Do not turn the editor into an importer replacement.
- This is an animation production workflow, not a general-purpose rigging,
  DCC modeling, motion-capture cleanup or video-editing application. Provide
  the runtime animation authoring/features listed here without rebuilding a
  full external DCC tool.

## Target asset and reference architecture

First audit and publish the repository's actual serialization schema and
ownership table. The target dependency graph is acyclic:

```text
SkeletalMeshComponent (one SceneDB instance)
  ├── AssetRef → SkeletalMesh → AssetRef → Skeleton
  └── AssetRef → Animation Graph → AssetRef → Skeleton
                         ├── graph nodes → Blend Space → Sequence AssetRefs
                         ├── graph nodes → Sequence AssetRefs
                         ├── graph nodes/slots → Montage AssetRefs
                         ├── graph nodes → IK Rig / Pose Database refs (as used)
                         └── graph nodes → reusable graph subgraphs/layers

Montage → Skeleton + ordered Sequence segment refs + slots/sections/notifies
Retarget Profile → source Skeleton + target Skeleton + chain/map configuration
IK Rig → Skeleton + bone-chain/goal definitions
IK Retargeter → source IK Rig + target IK Rig + retarget pose/settings
Pose Database → Skeleton + source Sequence refs + feature/index settings
```

The Skeletal Mesh defines its bound Skeleton. The component may show the
resolved Skeleton in its properties panel, but there must be one clear authored
authority: do not allow the component's Skeleton ref to contradict the Mesh
Skeleton. Keep a separate component override only if an existing engine
contract requires it, and validate exact compatibility on write/load.

Assets must never point to a spawned component or an editor preview instance.
An editor preview uses a transient component in a preview world. A Blueprint
class template holds component defaults and typed asset refs; each spawned
component instance resolves those refs and owns its own playback graph instance,
parameters, Montage instances, time cursors, notify-state cursors, curves,
root-motion delta, evaluated pose and previous pose. Runtime handles, pointers,
GPU allocations and cached loaded payloads are transient and not serialized.

Stable asset UUIDs are canonical. The project asset index resolves ID to path,
type, loaded SceneDB payload and diagnostics. No new global animation registry,
string-path identity, serialized reverse index or cross-DLL `TypeId` identity.
Asset pickers filter by asset kind and Skeleton compatibility, show name/type/
stable ID/path, offer navigation to the one canonical editor tab, and surface
missing, ambiguous, stale or incompatible references as repairable errors.

### Existing and new asset types

Preserve and deepen these existing file types:

| File | Single owner | Required scope |
|---|---|---|
| `.skeleton` | Skeleton asset/editor | Stable bone IDs, hierarchy, bind pose, sockets and bone metadata. Imported skeletons remain editable under the existing dependency-safety rules. |
| `.skmesh` | Skeletal mesh asset/editor/importer | Skin weights, joints, material slots, LODs and a typed Skeleton ref. Mesh editor remains focused on mesh preview/binding, not animation graphs. |
| `.animseq` | Animation Sequence asset/editor | Per-bone transform tracks and interpolation, curves, root motion, notify events and notify states with stable IDs and typed payloads. Preserve imported animation channels and precision. |
| `.blendspace` | Blend Space asset/editor | 1D/2D axes, sample Sequence refs, triangulation/interpolation, sample thresholds, per-sample playback/sync settings and preview inputs/weights. |
| `.montage` | Animation Montage asset/editor | Sequence segments, timing/rate/loops, named slots/tracks, sections/next links, blend in/out, interruption/group policy and montage-authored notify events/states. |
| `.animgraph` | Animation Graph asset/editor | Graphy-authored pose evaluation, state machines, transitions, graph parameters, reusable subgraphs/layers, slots, blend nodes, cached poses, IK/post-process and output. |

The current mockup spells the Montage path as `.animmontage`, but this checkout
registers and saves the extension `.montage`. Choose one canonical extension
based on existing registrations and references, update the mockup to match, and
provide an explicit alias/migration if compatibility needs the other spelling.
Never silently register two independent formats for the same asset.

Add these dedicated assets/editors because they own independently reusable,
typed authoring data that should not be copied into every Animation Graph:

| New file type | Purpose and editor | References |
|---|---|---|
| `.animretarget` | Retarget Profile Editor: source/target bone or chain mapping, translation/rotation modes, retarget pose, root and scale policy, validation and side-by-side pose preview | Source and target Skeleton refs; optional source/target IK Rig refs. |
| `.ikrig` | IK Rig Editor: bone chains, goals, retarget root, effector settings, solver stack/order and skeleton viewport overlays | One Skeleton ref. |
| `.ikretarget` | IK Retargeter Editor: source/target IK rigs, chain mappings/settings, target retarget pose, live side-by-side/overlay preview and per-chain diagnostics | Source IK Rig ref plus target IK Rig ref. |
| `.posedb` | Pose Search Database Editor: indexed source clips, sample interval, trajectory/pose feature channels, schema, exclusions, deterministic build status and query debug view | One Skeleton ref plus Sequence/Blend Space refs as supported. Generated search index is derived build data with version/hash; authored source settings remain in the asset. |

Introduce matching typed `AssetKind`s, file registrations, versioned codecs,
pickers, dependency reporting, validators, open/create menu entries and a
purpose-built editor for each new type. Do not make the Pose Database a hidden
cache or add asset types without substantive semantics and an editor. Do not
add a separate `.animlayer` merely to wrap a graph: represent reusable layers
as Graphy graph/subgraph documents or a substantive interface asset only if
the audit demonstrates independent reuse and ownership requirements.

## Runtime contract: SkeletalMeshComponent is the player

`SkeletalMeshComponent` is the scene-facing runtime wrapper, like Unreal's
skeletal mesh component plus animation instance. It is the one place where
resolved mesh/skeleton/animation refs meet engine lifecycle, component
events, SceneDB, physics and renderer data. Do not create a second
`AnimationPlayerComponent`, per-asset tick system or editor-only runtime.

On load/ref change, the component validates and resolves typed assets, builds
immutable/shareable evaluation data, initializes per-instance mutable state,
and reports broken refs without panicking. On each scheduled update it:

1. Receives gameplay parameters/commands written by Blueprint, input/gameplay
   code or animation events, with explicit frame/delta-time and deterministic
   ordering.
2. Advances the component's animation graph instance and active Montage/slot
   instances. Evaluate the base locomotion pose first, then layered slot and
   Montage poses with masks/priorities/blend weights, then additive and
   post-process/IK stages in documented order.
3. Produces one final skeleton-indexed pose plus curves, root-motion delta,
   notify crossings/states and debug evaluation trace.
4. Delivers notify and completion events through the per-component event
   channel with `ComponentRef`, source `AssetRef`, notify/instance stable IDs,
   playback time/cycle, phase (begin/tick/end for states), typed payload and
   relevant execution context. The event is not global.
5. Hands final pose/root motion to the established SceneDB transform/physics
   and GPU skinning paths. Publish stable current/previous bone palettes for
   rendering and the existing articulated physics consumer.

Tick/update phase and dependency ordering must be explicit in the engine
schedule. Handle pause, variable/fixed delta, rate scaling, negative/reverse
playback where supported, looping, seek/scrub (no gameplay event dispatch),
asset hot reload, missing refs, component removal and world teardown safely.
Use immutable shared source assets and avoid copying full clips per entity.
Cache per component only mutable evaluation state and compact derived data.

Physics authority must be defined: normal animation drives the skeleton;
physics can take ownership of configured bodies/chains and feed the resulting
pose back at a documented point. Prevent animation/physics write races and
preserve the pose consumed by rendering.

### Layering and gameplay behavior

The runtime and assets must support all of the following as one connected
pipeline, not disconnected feature demos:

- State machines with named states, entry/exit pose flow, rule graphs/typed
  conditions, transition duration/curve, priority, interruption policy,
  re-entry behavior, nested machines and deterministic state debug data.
- 1D and 2D Blend Spaces with validated axes, deterministic triangulation,
  normalized weights, edge/out-of-range policy, per-sample phase/sync handling
  and inspectable contributing samples/weights.
- Sequence players with time/rate/loop/sync control, additive and base-pose
  conversion, curves, root motion and source notifies.
- Montage playback with multiple simultaneous instances where allowed,
  named slot groups, per-slot blend/weight, segment/section routing, play,
  stop, pause, jump, set-next-section, rate, priority, interruption/group
  policy, blend in/out and deterministic completion results. A Montage overlays
  its slot pose onto the graph output; it does not replace the character's
  entire graph unless explicitly configured.
- Per-bone layered blending using named Skeleton masks or authored blend masks,
  additive poses, cached poses, linked/reusable animation layers and sync
  groups. Define curve propagation and root-motion ownership across layers.
- Animation curves with stable names/IDs, typed outputs and deterministic
  blending; map curve outputs to Blueprint/component/gameplay only through
  declared bindings.
- Root motion extraction and application modes with rotation/translation
  policy, loop seams, montage ownership and physics/network authority clearly
  defined.
- Notify events and notify states. Sequence and Montage records own their
  notifies. A state dispatches begin/tick/end exactly once per component
  playback instance across looping, reverse, interruption, seeking, hot reload
  and component destruction. Scrubbing a preview never emits gameplay events.
- IK Rig solver stacks for common runtime solvers (at minimum two-bone IK and
  FABRIK), goals, chains, effector transforms, joint limits/pole vector,
  retargeting chains and per-rig debug visualization. Animation Graph nodes
  apply configured rig/solver data to a pose; the `.ikrig` editor authors the
  reusable rig data.
- Retarget profiles between differing hierarchies, with chain correspondence,
  translation retargeting, root scaling, retarget pose and optional IK
  correction. Validate unmapped/ambiguous chains and show them in the editors.
- Motion matching/Pose Search using `.posedb`: deterministic database build
  from Sequence refs, authored feature schema, pose and trajectory matching,
  cost/debug breakdown, query history and runtime fallback. Rebuild invalidated
  derived indexes by version/hash and never write generated indices over
  source animation files.
- Bone/curve LOD, update-rate controls and pose caching policies that can be
  configured and profiled; correctness must not depend on renderer frame rate.

Do not mark an advanced item complete if its node, asset or editor is only a
stub. If an external engine or importer capability blocks a listed feature,
identify the exact dependency and stop that feature with a concrete integration
contract; keep the overall animation deliverable open until it has a real
runtime/editor path. Do not silently claim support or ship a node that returns
a constant/default pose.

## Editor plan: bind every workflow to the created UI

Use the real Pulsar editor's dark theme, dock/layout primitives, title/menu and
toolbar, left context tree, central viewport/canvas/timeline, right properties
inspector, asset tabs and status bar. The mockup is a layout/interaction
specification; native UI components and the engine's viewport/Graphy renderer
must implement it.

Every authored item uses a typed shared selection context. Selecting a bone,
key, notify, sample, graph node/pin, montage segment/section, retarget chain,
IK goal or pose-search result updates the same properties inspector. Inspector
edits write the owning asset/document, participate in undo/redo and dirty/save
state, and refresh preview/derived data. Navigation opens/focuses the one
canonical asset tab. Cross-editor refs link to the asset and never embed a
duplicate editable copy.

| Existing/mockup view | Owns data and visual surface | Real interaction and reference behavior |
|---|---|---|
| **Blueprint** | Existing Blueprint class/component template and gameplay graph; do not create another animation editor here | Component template uses typed Skeletal Mesh and Animation Graph refs, with resolved Skeleton displayed from the mesh. Gameplay nodes set graph parameters, play/stop Montage, jump sections and bind per-component notify/state/completion events. Event config includes component binding, source asset filter, stable notify ID and typed payload schema. |
| **Animation Graph** | `.animgraph` Graphy document and animation-specific graph profile | Graphy canvas for state machines, transitions and pose nodes; Graphy node visuals/interactions are reused. Add graph-specific state-machine view/transition editing without another renderer. Right inspector edits selected node/transition/parameter; lower preview shows pose, state, weights, curves, slots, root motion and trace. Asset refs open their source editor. |
| **Animation Sequence** | `.animseq` bone tracks, keys/interpolation, curves, root motion and notify/state records | Existing skinned preview + timeline; make notify/state lanes obvious with readable labels and generous hit areas at normal zoom. Add/move/delete/select keys/notifies/states; edit payload/config/time in inspector; loop, scrub, play, frame-step, zoom, filter tracks and bones. Saving must round-trip every notify field. |
| **Blend Space** | `.blendspace` axes, sample refs and sample positions | Purpose-built 1D strip or 2D plot with axes/grid/triangles/sample weights, sample list, selectable/draggable points, typed Sequence picker, validation and live parameter input/weight preview. Not a node graph. |
| **Animation Montage** | `.montage` segments, slots, sections/links, blend policy and Montage notifies | Multi-track timeline with Sequence segments, slot tracks, section markers/link visualization and event/state lanes. Select/reorder/trim segments, edit rate/loops/blends, link sections and preview layered slot output. Preserve the canonical file extension selected above. |
| **Skeleton** | `.skeleton` bones, bind pose, hierarchy, sockets | Extend existing hierarchy editor and viewport. Referrers are shown through a derived dependency index. Skeletons used by imported meshes/sequences remain protected by the existing dependency and bind-pose validation rules. |
| **Skeletal Mesh** | `.skmesh` geometry/skin weights/material slots/LODs and Skeleton binding | Extend existing preview/binding UI; show resolved skeleton compatibility, skin influence issues, LODs and asset refs. This is not the component runtime instance. |
| **Retarget Profile** | `.animretarget` chain/mapping/settings/retarget pose | Side-by-side source/target skeleton view, chain mapping table/viewport highlight, per-chain inspector and preview using shared sample sequences. Missing/ambiguous chains directly actionable. |
| **IK Rig** | `.ikrig` chains, goals and solver settings | Skeleton viewport overlays and hierarchy tree; create/select chains/goals; show solver order and editable effector/joint limits in inspector; preview pose changes. |
| **IK Retargeter** | `.ikretarget` source/target IK rigs, chain overrides and target pose | Side-by-side/overlay animated preview, chain status table, per-chain controls and target retarget pose tools. Opening source/target navigates to canonical IK Rig tabs. |
| **Pose Search Database** | `.posedb` source clips, feature schema, sampling/build settings; derived index is build output | Source clip table with ref picker, feature/trajectory channel authoring, build diagnostics/progress/version/hash, query timeline and cost breakdown, preview best matches and fallback graph. Never make this a generic JSON inspector. |

### UI-specific acceptance

- Add explicit pages/states to `docs/mockups/animation-editor.html` for
  Retarget Profile, IK Rig, IK Retargeter and Pose Search Database. Update the
  Montage extension shown by the mockup to the canonical registered extension.
  Keep the shared window chrome and make each central canvas/timeline distinct.
- The mockup must demonstrate the real cross-asset workflow: open Blueprint,
  choose BodyMesh's refs, navigate to its graph, edit a Blend Space sample,
  open a source Sequence notify, edit a Montage slot/section, then invoke that
  Montage on the same component binding and inspect its component-scoped event.
- Every control shown as clickable in the mockup must cause a meaningful model
  or preview change. Persist authored data; keep playhead, viewport camera,
  preview parameters, selection, active graph debugging and transient component
  runtime state out of the asset file.
- The Sequence notify lane must remain visible at normal zoom and dense marker
  collision. Provide collision stacking/lane offsets, hover, selected, disabled
  and notify-state range visuals; selecting it reveals full event configuration
  in the inspector. Never require external JSON editing.
- Use one editor selection/undo/save contract across these editors where
  practical. Use asset-type-specific panels and commands; do not force all
  asset kinds into a universal generic form or one mega-editor.

## Migration and delivery phases

### Phase 0 — Baseline and ownership audit

1. Read `AGENTS.md`, `.agents/CRATES.md`, `.agents/SCENEDB_MIGRATION.md`,
   `.agents/ENGINE_LIFECYCLE.md`, `.agents/REFLECTION.md`, `.agents/FRAMES.md`,
   `.agents/GRAPH_CANVAS.md`, `.agents/FILESYSTEM.md`, `POLISH.md` and the
   skeleton hierarchy docs. Inspect the actual Graphy API and plugin branch.
2. Inventory current asset schemas/codecs, SceneDB refs/index, runtime update
   scheduling, Blueprint notify event catalog/compiler, component property
   projection, import/migration, editor registration, preview/viewport,
   skeletal physics and every existing test/fixture. Record format versions and
   real extension/name differences.
3. Capture baseline timings on the same machine/build profile for 68-, 200-
   and 500-bone rigs, one/many active components, clips of several lengths,
   layered Montages, dense notifies and 1/2D Blend Spaces. Measure asset resolve
   and graph evaluation, animation evaluation, blending/IK, notify collection
   and dispatch, root-motion/physics handoff, CPU p50/p95, allocations/memory,
   GPU palette upload and editor timeline scrubbing separately. Record CPU,
   GPU, driver, OS, compiler, build flags and workload. If an instrument does
   not yet exist, build a reproducible harness before optimization.
4. Produce an ownership/ref DAG and a file/manifest/test migration map. Identify
   format migration, compatibility and submodule order. Do not begin feature
   implementation until this report identifies existing behavior that must
   stay intact.

### Phase 1 — Asset schemas, refs and compatibility

1. Define versioned authoritative schemas for existing assets and the new
   `.animretarget`, `.ikrig`, `.ikretarget` and `.posedb` assets. Keep authoring
   data separate from editor view data and transient runtime state.
2. Use Graphy's GraphDocument as the single mutable `.animgraph` graph model.
   Define animation node/pin types through the Graphy profile and type API.
   Keep animation-specific compiler/evaluator semantics in animation owners.
3. Add/refine typed AssetKind refs and project-index resolution for all asset
   types. Establish one Skeleton authority per mesh/component and compatibility
   diagnostics for every dependency edge.
4. Version and migrate current `.animgraph`, Blend Space and Montage JSON,
   retaining IDs/unknown fields as required. Resolve `.montage` versus the
   mockup's `.animmontage` naming with explicit aliasing if needed. Preserve
   `.skeleton`, `.skmesh` binary payloads and `.animseq` versions/notifies.
5. Add round-trip, old-version migration, missing/future version, broken ref,
   duplicate ID, incompatible skeleton and dependency-cycle tests before editor
   UI changes. Keep new file types substantive and registered end to end.

### Phase 2 — Component runtime and graph evaluation

1. Extend `SkeletalMeshComponent` authored state to select an Animation Graph
   while providing deterministic compatibility for existing direct Sequence
   playback. Migrate old `animation` references on decode; do not silently
   discard them or retain two independent active runtime systems indefinitely.
2. Implement a typed animation graph compiler/validator and deterministic pose
   evaluator over Graphy documents. Support graph parameters, state machines,
   transitions, sequence/blend-space players, cached poses, additive/layered
   poses, slots/Montage inputs, IK and output. Emit actionable graph diagnostics.
3. Give each component its own graph/player state; share immutable asset and
   compiled graph data across instances. Define parameter input API for
   Blueprint and gameplay code with stable names/types and validation.
4. Add Montage runtime instances, slot groups, blending/interruption/section
   commands and per-instance completion results. Support base locomotion plus
   upper-body Montage in one final pose and define mask/root-motion/curve
   precedence.
5. Complete retargeting and IK execution with profile/rig resolution,
   deterministic solver order and graceful diagnostics. Implement Pose Search
   build and runtime query if its asset/editor is included; do not expose a
   nonfunctional node.
6. Integrate evaluation with the actual SceneDB component schedule and Helio
   GPU/physics data paths. No editor-only animation tick, duplicate component
   player or unsynchronized GPU palette writer.

### Phase 3 — Notify, event, root-motion and physics semantics

1. Generalize existing per-component Sequence notify cursor/index from
   `skeletal_notifies.rs` to asset-source-aware playback instances spanning
   Sequence, Blend Space contributing clips and Montage segments.
2. Implement notify event and notify-state begin/tick/end behavior under
   forward, reverse, looping, high delta, section jump, stop/interruption,
   scrub/seek, pause, hot reload and teardown. Separate preview scrubbing from
   gameplay dispatch. Guarantee deterministic ordering and bounded work.
3. Deliver through the owning `SkeletalMeshComponent` event channel with typed
   source asset, notify ID, component ref, playback instance/cycle/phase and
   payload. Update Blueprint event discovery/compiler to consume canonical
   descriptors without global publication or per-open stale caches.
4. Define root-motion extraction/application, actor transform ownership,
   layered source selection, looping and physics authority. Ensure physics
   consumers see the intended evaluated/physics-corrected pose.
5. Test two components using the same assets with independent clocks and
   filters; one component's events, state, or interruption must never affect
   the other.

### Phase 4 — Production editors and UI integration

1. Implement the view matrix above in the existing skeletal animation plugin.
   Make each file kind open its own distinct editor experience and property
   context. Retain/extend its timeline and viewport renderer.
2. Replace the generic high-level asset form with bespoke Blend Space, Montage
   and Graphy Animation Graph views. Add purpose-built Retarget, IK Rig,
   IK Retargeter and Pose Search Database views.
3. Integrate SceneDB AssetRef pickers, breadcrumbs, canonical tab navigation,
   dependency tree, derived Used By query and incompatibility diagnostics.
   Update Blueprint component details/events using the same typed refs.
4. Make all controls functional, bind selected item to shared properties
   inspector, and wire all authored edits into undo/redo, dirty status and
   atomic save/reopen. Never let one editor save stale serialized state over
   an edit made by another tab; use the editor's document revision/dirty
   conflict mechanism.
5. Extend the standalone HTML mockup with all new editor screens and an
   end-to-end reference flow. Compare the native engine UIs at matching dark
   theme, panel sizes, zoom and selection states. The graph renderer should
   inherit Graphy's approved visual system, not reimplement it.

### Phase 5 — Validation, performance and handoff

1. Run crate/plugin tests, format migration fixtures, Graphy animation profile
   tests, Helio component/runtime tests, Blueprint compiler/event tests,
   importer tests and relevant root checks. Run `just check`, `just test` and
   CI commands appropriate to the changed dependency graph; state skipped GPU
   or platform tests clearly.
2. Run deterministic end-to-end flow in editor and runtime: choose component
   refs; preview locomotion graph and 2D blend; edit/save/reopen sequence
   notify/state; play Montage on that component; verify layered upper-body pose,
   section jump, root-motion result, per-component event payload and physics/
   GPU pose agreement.
3. Round-trip every current and new format and test upgrade from real fixtures.
   Verify IDs remain stable, unknown supported fields survive, reverse
   dependency queries update and no circular serialized refs are introduced.
4. Capture native UI screenshots for every editor plus Graphy node zoom,
   transition selection, dense notify collisions, active Montage layers,
   Blend Space boundaries, retarget failures, IK goals and Pose Search query
   debug. Review against the standalone mockup and document gaps.
5. Repeat Phase 0 metrics on the same machine/profile/workload. Publish
   before/after tables with p50/p95 and percent change for every metric. Explain
   regression, wins and limits; do not fabricate stats, compare different
   hardware or treat LOC reduction as performance evidence.
6. Update `.agents/CRATES.md` and animation/SceneDB/Blueprint docs, manifests,
   asset registries, plugin editor registration, import compatibility, CI and
   submodule pins. Report exact files/crates added, deleted, moved and retained;
   explain every adapter and give its removal condition.

## Required completion checks

- Every listed asset type has one substantive owner, typed stable identity,
  versioned codec, dependency validation, editor registration and tests.
- `SkeletalMeshComponent` remains the runtime player/lifecycle/tick owner and
  uses SceneDB refs. Authored assets do not contain component handles or
  serialized reverse refs. Multiple components play the same assets
  independently.
- Animation Graph is a Graphy document with one shared renderer and
  Graphy-managed graph interactions. No fourth private Blueprint-like editor
  or generic synchronized graph copy remains.
- Blueprint can set graph parameters, play/jump/stop Montages and subscribe to
  asset-owned notify events/states per component with typed source refs.
- Sequence, Blend Space and Montage editors show distinct visualizations and
  selection/inspector models matching `docs/mockups/animation-editor.html`;
  new Retarget/IK/Pose Search screens are also represented in the mockup.
- Runtime supports layered base + slot/Montage pose, state machines,
  transitions, blend spaces, curves, root motion, notify states, retargeting,
  IK, physics handoff and Pose Search as documented; no visible control/node
  is a nonfunctional placeholder.
- Existing asset/import data round-trips and migrations are proven by fixtures.
  Preview uses the real component runtime path without serializing preview
  state or firing gameplay events during scrubbing.
- Performance baselines and post-change stats are reproducible and reported
  for runtime, references, notifies, graph and editor timeline workloads.
- No issue closure, external comment, commit, push, merge or release occurs as
  part of code implementation unless the user separately authorizes it.

## Human-readable scope checklist

1. Preserve `.skeleton`, `.skmesh`, `.animseq`, `.blendspace`, `.montage` and
   `.animgraph`, updating the last three into a complete system.
2. Add `.animretarget`, `.ikrig`, `.ikretarget` and `.posedb` with typed refs,
   real editors and runtime/validation ownership.
3. Build full pose evaluation and layering inside `SkeletalMeshComponent`:
   graph/state machines, blend spaces, sequences, Montage slots, masks,
   additive layers, curves, root motion, IK, physics and motion matching.
4. Make Sequence/Montage notifies and notify states authored, selected,
   inspectable and saved in their source assets, then dispatch them only to the
   correct component instance and Blueprint binding.
5. Build the distinct editors and cross-asset workflow shown by the mockup;
   reuse Graphy for graph UI and SceneDB for reference identity/resolution.
6. Migrate safely, test the end-to-end character workflow and publish real
   before/after performance measurements before calling the system complete.
