# Frames — parented transforms, rigid frames, sublevels (design)

Status: phases 0-7 are built to the extent described in "As built"; the rest is **design**. "As built" describes what exists; everything
under "Design" and "Phases" is proposed and not implemented. No code is
specified here beyond what is needed to name data.

## Why

Grouping objects in the hierarchy must move the group's contents with it, and
the renderer must be able to do that cheaply. Most engines resolve this on the
CPU (every descendant's world transform is rewritten when a parent moves). We
already have a GPU-driven scene (SceneDB columns mirrored to GPU, joins that
turn them into draw rows), so the hierarchy belongs there, and it should be the
same mechanism sublevels, rooms, ships and portals use, not a parallel system
next to the existing coordinate-space table.

Goals:

- Moving a group costs O(1) CPU writes however many objects it holds.
- Moving a *rigid* group (a ship, a room, a sublevel) also costs O(1) GPU work:
  its members are not rewritten.
- Membership is dynamic: reparenting an object into a ship changes how it
  renders with no CPU re-tagging.
- Positions stay precise far from the origin (planet-scale scenes).
- One authored model. The editor's folders, the runtime's sublevel actors and
  portal targets are all the same kind of thing.
- No SceneDB fork for the first phases; the generic parts of SceneDB stay
  generic.

Non-goals: skeletal/bone hierarchies (they have their own palette path),
physics-joint hierarchies, non-rigid (sheared) frames.

## As built

Parent-relative transforms, resolved on the GPU. Introduced with the folder
grouping work.

- `Transform` (`pulsar_scene_model::components`) is **relative to the object's
  `Parent`**, or to the world when it has none. A flat scene is unchanged.
- `ObjectParent` (`components.rs`): a derived packed GPU row (`object_parent`,
  one `u32` per entity: parent entity index + 1, 0 = none), derived from
  `Parent` the same way `ObjectHidden` is derived from `Visibility`.
- `HierarchyResolve` (`helio-default-passes/src/hierarchy_resolve.rs`, shader
  `shaders/hierarchy_resolve.wgsl`): a `SceneDerivation` installed **before**
  the scene and environment joins. Per object it walks the parent chain
  (cap 64) in one compute thread and writes the composed world transform.
  It republishes the result **under the same `Transform::packed` key**, so the
  scene join, environment join and any other reader of that key see world-space
  rows and know nothing about hierarchies. It does nothing until the parent
  column has ever been written (`content_generation == 0`), so flat scenes keep
  every existing fast path.
- `SceneBufferProjection::insert` drops the flushed-row record of a replaced
  buffer, because a parent's move changes rows the flush never named.
- CPU twin: `pulsar_scene_model::hierarchy_transform` — `world_transform_of`,
  `world_matrix_of`, `set_world_transform`, `local_for_world`,
  `reparent_keeping_world`, `descendants_of`, legacy conversion. Same Euler
  convention (YXZ, degrees) as the shaders; verified GPU-vs-CPU by
  `the_gpu_composes_a_parent_chain_like_the_cpu`.
- Editor: the gizmo works in world space and writes local values;
  `reparent_object` keeps the world pose; scene files are version `2.2`
  (parent-relative). Older files are converted on load
  (`file_has_world_space_transforms`).
- Classes: a generated child's `Transform` is its slot's local transform; it
  follows its root with no re-layout.

- Phase 1 (precision): rows are published **relative to the frame's world
  origin**. The topmost ancestor's position has the origin (hi/lo pair) removed
  before anything is added, so nested content far from the origin keeps the
  precision of its offset. The resolve publishes the empty marker buffer
  `TRANSFORMS_ORIGIN_RELATIVE_KEY`; `SceneJoin` and `EnvironmentJoin` then skip
  their own origin subtraction (`join_origin`). Tests:
  `nested_objects_far_from_the_origin_keep_their_offsets`,
  `a_flat_scene_is_still_made_origin_relative_by_the_join`.
- Phase 2 (dirty-subtree propagation): SceneDB's flushed-row records of the
  `Transform` and `object_parent` columns seed a changed set (`cs_mark`); every
  row then checks its chain for a changed ancestor (`cs_resolve_moved`, reads
  only) and only those rows are recomposed and appended to a GPU-built moved
  list in the same `[count, rows..]` layout SceneDB uses, with indirect
  dispatch args (`HIERARCHY_MOVED_ARGS_KEY`). The list is attached to the
  published buffer through the new `SceneDerivationOutput::flushed` /
  `SceneBufferProjection::set_flushed_rows`, and `SceneJoin`'s moved-owner join
  consumes it (indirect dispatch; the "large share moved" shortcut is skipped
  because the count is not known on the CPU). A full resolve happens on the
  first frame, after reallocation, an origin change, or any gap in a column's
  flush sequence. Test: `hierarchical_scenes_follow_only_what_moved` (child
  move, folder move, reparent; asserts the moved-owner path is taken).

Remaining limits of the as-built resolve:

1. **The scan is O(rows), the rewrite is O(moved).** Each frame with a change,
   every row reads its ancestors' changed bits. That is cheap, but not
   O(moved); child lists (or frames, phase 3) are needed for that.
2. **Camera-relative frames re-resolve fully.** When the frame origin moves
   (the camera moves in a relative-camera scene) every row is recomposed, as
   the joins already redo every row in that case.
3. **Top-ancestor storage precision.** A root's `Transform` position is f32; the
   origin subtraction cannot recover digits lost in storage. Content that is
   both far away *and* large belongs in a frame (phase 3, f64 origin).
4. **Origin low half on some GPUs.** `relative_position` is
   `(position - hi) - lo`. On this machine's GPU the compiler drops `lo`
   (`camera_relative_frames_get_positions_relative_to_the_world_origin` in
   `helio-default-passes/tests/scene_join.rs` fails identically on the
   committed Helio, before this work: 2.5 instead of 2.375). Up to ~0.25 m at
   planet radius. Pre-existing, affects every camera-relative consumer, not
   just the resolve; needs a shader fix that defeats reassociation.
5. Shear is not representable in `Transform`; a rotated, non-uniformly scaled
   parent over a rotated child resolves to the nearest shear-free transform.

## Existing sublevels (what we are unifying with)

- `SubLevelActorComponent` (`helio-pass-gbuffer/src/components.rs`): index of a
  sublevel + a placement matrix. Sublevels are *indexed content spaces*, not
  SceneDB entities.
- `SubLevelResolver` / `PortalProjectionBridge`
  (`helio-pass-portal-cull`): CPU resolution of actor trees and portal chains
  into a coordinate-space table.
- The coordinate-space table: ≤ 32 rigid `Mat4` slots, slot 0 identity. Each
  instance carries an 8-bit space id in its flags and is drawn through
  `coordinate_spaces[id] * model` in the vertex shader. Moving a space is one
  matrix; members are untouched. The previous frame's table exists for motion
  vectors.
- Portal-cull duplicates draws per portal *chain* (composed transforms), so
  "same content, several placements" machinery already exists on the GPU.

Strengths to keep: O(1) move of arbitrarily large content, no per-member work,
small coordinates inside a space. Weaknesses to remove: membership is a CPU
tag set at resolve time, 32 slots / 8 bits, one level of tag so nesting is
flattened by the CPU, and none of it is reachable from the editor's folders.

## Design

### Two tiers, one authored model

`Parent` stays the single authored relationship for everything.

- **Tier 1 — transform groups.** An ordinary parent (a folder). Members are
  stored relative to it and the resolve composes them. Cheap to author,
  arbitrary nesting, any object can be a parent. Cost scales with the size of
  the group *when it changes*.
- **Tier 2 — frames.** A parent carrying a `Frame` marker (explicit in the
  editor: "make rigid frame"; implicit for sublevel actors and portal targets).
  A frame is a rigid transform (translation f64, rotation, uniform scale)
  with one row in a growable **frame table**. Everything beneath it is stored
  relative to it and is **never rewritten when the frame moves**: the frame's
  matrix is applied once, at join/vertex time.

The resolve composes a chain only **up to the nearest frame ancestor**. So
chain length is bounded by the local group (typically a handful), not by the
scene, and a frame moving costs one table row.

### Dynamic space ids

The resolve also writes, per object, the id of its nearest frame ancestor
(0 = world). This replaces the CPU tagging of instance flags: reparent an
object into a ship and its space changes on the next resolve, with no CPU
walk. Instance rows read the space id from the object (the join already
fetches the owner row per instance).

### Frame table

- CPU: each frame has an f64 origin (so a frame can sit anywhere on a planet)
  plus rotation and scale. Frames nest (a room in a ship), composed on the CPU
  in f64 — frames are few.
- GPU: one row per frame, **camera-relative f32** (origin subtracted in f64
  first, the existing hi/lo `world_origin` convention), plus last frame's row
  for motion vectors. Widen the id past 8 bits/32 slots; the table grows like
  other mirrored columns.
- Members of a frame hold small f32 coordinates relative to the frame, which
  is where the precision benefit of "local space" comes from.

### Precision (phase 1, built)

Compose camera-relative. The resolve receives the frame origin
(`SceneDerivationContext::world_origin`, hi/lo pair). The topmost ancestor of
a chain (a root object or a frame) has its position reduced by the origin
*before* anything is added; children then add small offsets. Output rows are
camera-relative, and the joins skip their own origin subtraction for rows the
resolve produced (an explicit flag on the published buffer, not an implicit
convention). Result: error is bounded by the size of the group, not the
distance from the origin. For content that is far away *and* large, it
belongs in a frame, whose origin is f64.

Test: a parented group at x ≈ 6.4e6 m, children at 0.01 m offsets, resolved
and drawn — assert child separation survives; compare with the as-built
resolve to show the regression it fixes.

### Dirty-subtree propagation (phase 2, built)

Goal: a single object move in a hierarchical scene costs one row, and a group
move costs its subtree, not the scene; and the join's moved-owner fast path
works again.

Per resolve, from the transform and parent flush records:

1. Seed a changed set from the rows SceneDB flushed this frame (Transform and
   Parent columns both).
2. Each object checks its chain for a changed ancestor (reads only, chain is
   short because of frames); changed or changed-ancestor objects are the moved
   set.
3. Only moved objects are recomposed; the rest keep last frame's world rows.
4. The moved set is published as the flushed-row record for the world
   transform buffer, so `SceneJoin` takes its moved-owner path.

Decision (made, option (a)): `FlushedRows` carries a CPU-side count
today (`record_moved` takes it by reference). Either (a) the resolve produces a
GPU list and the moved-owner join gets an indirect variant (the join already
builds GPU moved lists, `scene_objects_moved`), or (b) the CPU maintains child
adjacency from the Parent change journal and expands flushed rows into a
subtree list. (a) keeps the O(1)-CPU property and is the recommendation; (b)
is simpler but is O(descendants) on the CPU per group move.

### Placement and instancing (sublevels)

A sublevel actor becomes a frame whose members are *content-space* rows. The
same content placed N times is one set of content rows plus N frames — the
rows are not copied. A **placement expansion** stage generalises portal-cull's
draw duplication per chain:

1. Frustum/occlusion-cull each *placement* (a frame's content bounds) first.
2. Expand surviving placements into instance rows tagged with the frame's
   table slot.

So off-screen sublevels cost one bounds test, not one test per member. Portal
chains are frame-to-frame transforms in the same table; chain composition and
frame composition become the same code.

### Content-space partitions in SceneDB

Do **not** make sublevels separate `World` objects: the GPU mirror, the joins
and all queries assume one world, and per-world mirrors would defeat batched
GPU-driven draws. What sublevels do need from SceneDB is generic:

- a partition key on rows ("content space N"), with per-partition
  load/unload/streaming, change journals and mirror flush ranges;
- cross-partition references as `(space, stable id)`.

This is the only phase that needs a SceneDB change (separate repository,
dependency-first PR and re-pin per "Submodule pins" in `CRATES.md`). Separate
`World` instances remain right for hard isolation, e.g. the play-in-editor
world.

## As built: frames, sublevels, portals, partitions

Phases 3-7 below are built to the extent described here; "not done" lists what
is deliberately left, with the reason.

**One allocator for every kind of space.** Rigid frames, sublevel placements and
portal projections are different authored things with one runtime need: a slot
in the coordinate-space table. None of them numbers the table, and none knows the
others exist.

- *Authored primitives stay the only authored things:* `PortalComponent`,
  `SubLevelActorComponent`, and the `rigid_frame` object prop (which becomes the
  `RigidFrame` marker). There is no separate "frame" component to author for a
  portal or a sublevel.
- *Anything that defines a space registers a `SpaceDefiner`* (`inventory`) in
  the crate that owns the component: `pulsar_scene_model::frames` registers
  `RigidFrame`, `engine_backend::scene::sublevels` registers
  `SubLevelActorComponent`. A definer names its objects and may supply a
  normalizer (a sublevel placement's authored matrix is folded into the object's
  `Transform` once, so a space has exactly one placement).
- *`sync_spaces(&mut World)` is the allocator.* It gives every claimed object a
  `SpaceSlot` (`1..=255`), removes it when the claim ends, runs normalizers, and
  is idempotent; `needs_space_sync(&World)` is its read-only check.
  `SpaceSlot` is mirrored to the GPU as the derived `ObjectFrame` row.
- *Entity-less spaces take slots through `helio_core::CoordinateSpaceAllocator`.*
  `engine_backend::scene::SpaceTable` implements it over the table the objects'
  spaces already fill; `PortalProjectionBridge::build_into(resolver, &mut table)`
  asks it for each projection's matrix (`build` keeps numbering its own table, for
  hosts without frames). `Renderer::set_coordinate_spaces` takes the one result;
  `set_portal_projection_counts` carries the portal view and chain counts.

**Frames (phase 3).**

- `pulsar_scene_model::frames`: `RigidFrame`, `SpaceSlot { slot }`,
  `SpaceDefiner`, `sync_spaces`, `make_frame` / `clear_frame` /
  `set_rigid_frame`, `frame_table(world, origin)` (every space's world matrix
  composed in **f64**, handed over relative to the frame origin as f32),
  `frame_above`, `space_of`. A rigid frame is authored with the object prop
  `rigid_frame: true` (`FRAME_PROP`); `apply_frame_prop` makes the component match
  it on `add_object`, `update_object` and runtime load, so it survives save/load,
  undo and duplicate with no file-format change.
- `HierarchyResolve` publishes **two** results: the world rows
  (`Transform::packed`, for lights, environment volumes, anything not
  space-aware) and the **placement** rows (`Transform::placement`) composed only
  up to the nearest space, with the object's space in `object_space`. Placement
  rows are not origin-subtracted inside a space (they are small). Dirty tracking
  has three bits (transform / parent / slot); a space moving adds nothing to the
  placement moved list.
- `SceneJoin` places meshes from the placement rows, writes the space into the
  instance flags (bits 8-15, which the G-buffer, occlusion cull, shadow, shadow
  cull, shadow dirty and ray-tracing passes already honour), and lights keep the
  world rows. Its structure signature includes the space buffer, so changing a
  parent link or slot re-sorts, while moving does not.
- `GBufferPass`: 256 table slots (was 32) and a real previous-frame table (it
  used to upload the current table as "previous", so moving spaces had no
  motion vectors; a table that stops changing catches `prev` up next frame).
- Engine: `SpaceSync` (`engine_backend::scene`) runs the allocator when
  `needs_space_sync`, builds the `SpaceTable` each frame and uploads it only
  when it changes, in the editor renderer and both game render paths. Editor:
  **Make Rigid Frame / Make Ordinary Group** in the hierarchy context menu (an
  undoable `UpdateObject`).
- Tests: `frame_members_are_placed_in_their_frame_and_follow_the_table` (GPU,
  nested spaces, `table[space] * row == CPU world matrix`, moving a space
  rewrites no member row), `a_rigid_frames_meshes_move_with_it_in_the_frame`
  (full frame through the editor commands), `independent_definers_share_one_
  allocator`, the table/prop unit tests.

**Sublevels (phase 4).** A placement is just a registered definer (see above):
no adopt step, and the component is the only thing authored. Test:
`a_placement_is_a_space_holding_its_contents`, `a_placement_and_a_rigid_frame_get_
different_slots`.
**Not done:** the CPU `SubLevelResolver`'s own use of the table for sublevel
content is untouched (nothing in the engine feeds it); a level-asset format for
sublevel content, an editor UI to place one, instancing one content several
times (each placement holds its own objects today), and per-placement culling.
These need authoring decisions first.

**Portals (phase 5).** Portals author only `PortalComponent`. Their per-projection
spaces come from the same allocator as everything else (`build_into`); there is no
reserved range and no layering in the renderer. Test:
`frames_and_a_portal_projection_share_one_table_without_overlap` (engine), and
`a_host_allocator_shares_its_table_with_the_projection` (Helio). **Not done:**
portal chain composition does not yet reuse frame composition (they are still two
code paths).

**Partitions (phase 6).** `pulsar_scene_model::partitions`: `ContentSpace(n)`
(prop `content_space`), `assign_space`, `objects_in_space`, `space_roots`,
`spaces_present`, `unload_space`. **Not done:** the SceneDB-level partition key
(per-partition journals and mirror flush ranges) lives in the external SceneDB
repository and is a separate dependency-first PR.

**Static content (phase 7).** A `Static` object inside a rigid frame is fixed
only relative to the frame, which moves, so the join marks it **movable** in the
world (`INSTANCE_FLAG_MOVABLE`): static shadow casters and ray-tracing instances
are keyed to a fixed world transform and would otherwise go stale. The editor's
drag warning now covers a plain group holding fixed objects
(`first_fixed_in_group`); a rigid frame's members are exempt. Test:
`static_meshes_inside_a_frame_are_movable_in_the_world`, `group_motion.rs`.

## Phases

0. **As built** (done).
1. **Precision** (done): camera-relative resolve; joins skip re-subtraction for
   resolved rows; far-from-origin test.
2. **Dirty-subtree propagation** (done; GPU list chosen, option (a)): moved set, flushed-row publication, restore
   the moved-owner fast path in hierarchical scenes. Decide (a) vs (b) above
   first. Benchmark against `movement_scaling` for a flat scene (must not
   regress) and a 100k-object group.
3. **Frames** (done): `Frame` component, growable frame table (f64 CPU, camera-relative
   GPU, previous-frame row), resolve stops at the nearest frame and writes
   space ids, instance path reads space id from the object. Replaces the
   8-bit/32-slot table and CPU instance tagging. Editor affordance to promote a
   folder to a frame.
4. **Sublevels on frames** (foundation done, see above): `SubLevelActorComponent` placements become frames;
   delete `SubLevelResolver`'s tagging; placement expansion stage with
   per-placement culling.
5. **Portals on the frame table** (table sharing done, see above): portal chains compose frames; one code
   path for chains and frames.
6. **Content-space partitions** (scene-model half done; SceneDB half is a separate PR).
7. **Static content in moving frames** (done): "Static" currently means caches (shadow
   pages, ray-tracing instances) may be keyed to a transform that never
   changes. Under a moving frame, caches must be keyed in frame space and the
   frame matrix applied when sampled.

Each phase ships with: a CPU-vs-GPU equivalence test (as
`the_gpu_composes_a_parent_chain_like_the_cpu`), a full-frame acceptance test
through the editor commands (`render_acceptance.rs`), and the flat-scene
regression tests unchanged.

## Risks and open questions

- **Static meshes** under a moving frame (phase 7) — the biggest semantic one.
- **Shear:** frames are rigid + uniform scale. Non-uniform scale lives only in
  short within-frame chains; reparenting a non-uniformly scaled object under a
  rotated parent can change its shape (nearest shear-free transform). Decide
  whether the editor warns or refuses.
- **Bulk CPU consumers** (physics sync, scripts, AI queries) need world poses.
  `world_transform_of` is O(depth) per query; a batched path is needed before
  physics bodies are allowed under moving frames. The physics plugin currently
  reads the raw (now parent-relative) `Transform`; bodies under groups are
  therefore unsupported until it uses the world pose.
- **Scripts:** `Transform::position` and friends are parent-relative. Document
  it; add world-space accessors before relying on them in grouped content.
- **Undo of gizmo drags** records the dragged object's own (relative)
  transform; contents are never rewritten, so one entry is correct for a whole
  group — keep that property.
- **Depth cap (64)** and cycle handling: reparenting already refuses cycles;
  the cap only bounds a corrupt world.
- **Multiuser editing** (`MULTIUSER_EDITING_SPEC.md`): a frame move is one row
  and replicates as one change; partitions are a natural unit for ownership
  and locking. Worth reading together with phase 6.
- **Industry comparison:** most engines propagate parent changes to descendants
  on the CPU (dirty-flag hierarchies, attach propagation). GPU-resolved groups
  with dynamic frame membership, placement-level culling and f64 frames is the
  intended differentiator; claims should be backed by the phase-2 and phase-4
  benchmarks before being made anywhere public.

## Issues for the cleanup pass

Known, not fixed. Ordered roughly by how much they matter.

1. **Origin low half dropped on some GPUs** (pre-existing). `relative_position`
   `(position - hi) - lo` loses `lo` on this machine's GPU:
   `camera_relative_frames_get_positions_relative_to_the_world_origin`
   (`helio-default-passes/tests/scene_join.rs`) fails identically on the
   committed Helio. Up to ~0.25 m at planet radius, for every camera-relative
   consumer. Needs a shader formulation the compiler cannot reassociate.
2. **Physics and scripts read parent-relative `Transform`.** Bodies and scripts
   inside groups or frames see local values until they use
   `hierarchy_transform::world_transform_of`. Bulk consumers need a batched
   world-pose path.
3. **Hiding a folder.** `ObjectHidden` is per object; whether hiding a group
   should hide its contents (and how a frame's contents follow) is not decided.
4. **Sublevel authoring and instancing** (phase 4): level-asset format, editor
   UI, one content placed several times, per-placement culling. `SubLevelResolver`
   is still unused by the engine.
5. **Portal chains do not reuse frame composition** (phase 5): two code paths (slots are shared; composition is not).
6. **SceneDB partition key** (phase 6): per-partition journals and mirror flush
   ranges, in the external SceneDB repository.
7. **A frame moving still advances the placement buffer's generation**, so the
   join runs its moved-owner path with an empty list each time. Cheap, not free;
   skip when no placement moved.
8. **The changed-bit scan is O(rows)** per frame with a change (the rewrite is
   O(moved)). Child lists would make it O(moved).
9. **Camera-relative frames re-resolve every row** whenever the origin moves.
10. **255 frames** (space id is 8 bits of the instance flags).
11. **Shear:** a rotated, non-uniformly scaled parent over a rotated child
    resolves to the nearest shear-free transform; the editor neither warns nor
    refuses.
12. **Grouping undo** is one step per moved object plus one for the folder.
13. **No visual indicator** in the hierarchy for a rigid frame (menu toggle
    only), and no inspector field.
14. **Voxel worlds** still require an unparented, unrotated owner.
15. **Motion vectors** for frame members use the new previous-frame table but
    have only been checked by construction, not visually.
16. **Unverified neighbours:** `ui::viewport::tests::cursor_capture_load_
    resolves_legacy_conflict_to_rotation` fails and the code is not touched by
    this work (its pre-change state was not confirmed);
    `properties_panel_layout_budget` fails only in parallel runs.
17. **Industry claims** in this document are unverified; back them with the
    phase-2 and phase-4 benchmarks before repeating them.

### Portal findings (lighting + cost pass)

- **Portal lighting**: portal pixels are tagged `(-1, -3 - chain)` in the G-buffer
  `lightmap_uv`; `deferred_lighting.wgsl` maps position/normal/eye back through the
  chain (`portal_chain_unmap`, a 256-entry uniform in group 0 binding 11, published by
  `PortalProjection`) and walks all scene lights (cap `PORTAL_MAX_LIGHTS`) instead of
  the host tile lists. Shadows are sampled at the content's real position, so content
  outside the host camera's shadow cascades/local shadow maps is unshadowed. SSR and
  planar reflections are skipped for portal pixels.
- **Portal cost**: chains were built for every path (depth 3) whether or not the
  camera could see them (`build_into`); every chain redraws the scene it shows. The
  editor now builds chains with `build_visible_from_camera_into` (`SpaceSync::set_view`).
  `pulsar_game` (`embed.rs`, `windowed_app.rs`) still calls `sync` without a view, so
  it builds every path: give it the camera too. Not yet measured on a GPU.
- **Portal materials**: portal duplicates now draw per material key like the G-buffer
  (`PortalCullPass` packs `portal_indirect` into portal segments laid out by the
  G-buffer's segment table via `compact_portal.wgsl`; `PortalInstancePass` draws one
  segment per key with a pipeline built from `gbuffer_portal.wgsl` plus the graph,
  `RadiantTemplate::build_shader_source_on`). Graph `time` nodes read 0 there; the
  transparent and forward buckets are not drawn through portals; built-in template
  classes 1-4 follow the G-buffer (which also only uses class 0). Compiles for graph
  and built-in classes are unit-tested; a graph material drawn through a portal on a
  GPU is NOT yet verified.
- **Not a bug (was a test artefact)**: "content shifts with portal distance" came from
  the test scene (mesh radius ~173 m, so the far content was directly visible, and the
  near portal faced away from the camera). Portal tests must face the near portal at the
  camera (`rotate_y(.., 180.0)`) and keep the far portal thousands of metres away.
