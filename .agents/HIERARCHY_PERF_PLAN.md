# Hierarchy (Outliner) Performance Plan

Status: plan only. Wave 0 landed; waves 1-5 are not started.

## The problem

When an object was selected in the viewport, the editor froze for about 2 s
(profiled: `hierarchy: ECS snapshot` → `scene_edit::get_hierarchy_projection`).
Selecting one object should cost about what it costs to repaint the rows that
changed. That means O(rows on screen) or O(objects that changed), never
O(objects in the level), and certainly not O(n²).

Mature editors cost this little because they never rebuild the outliner from
the whole scene on an event:

| Engine | Outliner model | How selection works | How it updates |
|---|---|---|---|
| Unreal (Scene Outliner) | A persistent tree of `FSceneOutlinerTreeItem`s, indexed by id | `USelection` is editor state, not a world mutation | Hierarchy events (actor added, removed, attached) queue pending operations, which are applied in batches each tick. Only `STreeView` rows on screen get widgets |
| Unity (Hierarchy window) | Native `Transform` keeps a children array; the window keeps a `TreeView` model | `Selection.objects` is editor-side and changes no scene data | It rebuilds only on `hierarchyChanged` and patches the rows involved. Only visible rows are drawn |
| Godot (SceneTreeEditor) | A `Node` owns its ordered children; `TreeItem`s map 1:1 to nodes | Editor selection is a separate set | Per-node signals (`child_entered`, `renamed`, ...) patch single `TreeItem`s |

All three:
1. store parent → ordered children as data, so nothing has to scan to find it;
2. keep selection separate from the "the scene changed" signal;
3. keep a persistent row model and patch it from fine-grained change events;
4. build UI only for the rows on screen.

Pulsar currently does none of these. The cost comes from several stacked
layers, which the waves below remove one at a time.

## The cost layers today

1. **`children_of` / `entity_for` are full scans** (`pulsar_scene_model/src/world_ext.rs:138,152`).
   There is no id index and no children index. About 228 call sites across
   `crates/` and `plugins/` use them, many inside loops. *(Partly fixed for the
   projection in wave 0.)*
2. **Selection bumps the world revision.** `select()` inserts and removes the
   `Selected` component, so the hierarchy cache (keyed on `World::revision()`)
   treats every click as a structural change. Every other panel keyed on the
   revision rebuilds as well.
3. **Every rebuild covers the whole scene.** `refresh_cache` rebuilds every
   row's projection and `SceneObjectItem` (strings, child-id `Vec`s, icon
   lookups, class role checks), even though nothing structural changed.
4. **The shared tree widget is quadratic as well**
   (`wgpui-component/.../hierarchical_list_view.rs`):
   - `get_root_item_indices` calls `root_ids.contains(..)` for each item. That
     is O(n·roots), and roughly n² for a flat level.
   - `find_item_index` / `find_item` do a linear search for each expanded
     child.
   - `deduplicate_hierarchy_ids` is O(k²) per node.
   - `is_expanded` takes the `LevelEditorState` read lock once per item.
5. **Everything is copied on each render.** `cached_items.clone()` copies
   every row on every render. Ids are `String`, so they are cloned into keys,
   child lists, closures and selection sets.

## Wave 0 (landed, 2026-11-16): projection O(n²) → O(n)

- `get_hierarchy_projection` does one `StableId` query to build the
  parent → children map, then walks it depth-first.
- Each row carries its `Entity`, so the class role and movability checks no
  longer call `entity_for`.
- The object icon registrations are sorted once per rebuild, not once per row.

What is left: the cost of a selection is still O(n). It is linear now
instead of quadratic, but it should not depend on n at all.

## Wave 1: remove the quadratic code from the tree widget (small, isolated)

`hierarchical_list_view.rs`:
- Build `HashMap<Id, usize>` once per render, and a `HashSet` of root ids.
- Replace `find_item_index` / `find_item` / `get_root_item_indices` with
  lookups in those maps.
- Make `deduplicate_hierarchy_ids` use a `HashSet`, or drop it once the source
  guarantees unique children.
- `is_expanded`: give the widget a snapshot of the expanded set (`Rc<HashSet>`)
  instead of a closure that takes a lock on each call.

Every hierarchy that uses this widget benefits, including the file manager
and other trees. Done when: flattening is O(visible rows + n) with no nested
scans, and an existing tree view looks unchanged.

## Wave 2: selection is not a structural change

Choose one approach (A is recommended):

- **A. Split the cache key.** Key the structural cache on the change journals
  of the components that define the tree: `StableId`, `Parent`,
  `SiblingIndex`, `Name`, `ObjectType`, `Visibility`, `RenderProps` (icon),
  and the class and instance markers. Use
  `World::open_change_cursor::<T>()` + `read_changes`, the journal SceneDB
  already ships. Changes to `Selected` are not part of the key. Selection
  becomes a per-row flag read at paint time from a small `HashSet` (the
  `Selected` query plus `hierarchy.multi_selected`). That makes selection
  O(visible rows).
- B. Move selection out of the world (an editor-only `SelectionSet` like
  Unreal's `USelection`). This is more invasive: the renderer outline,
  scripting and undo read `Selected`. Revisit only if A leaves other panels
  still rebuilding on selection.

Also audit the other consumers keyed on `world_revision()` (properties,
viewport overlays) for the same false invalidation.

Done when: a selection click shows no `hierarchy: ECS snapshot` span at all.

## Wave 3: real indexes in the scene model (fixes about 228 call sites at once)

In `pulsar_scene_model` (or SceneDB, if it should be generic):
- **Id index:** maintain `HashMap<StableId, Entity>` where a `StableId` is
  inserted or removed, so `entity_for` becomes O(1). This could be a SceneDB
  "unique index" over a component, or maintained by the world extension's
  spawn and despawn paths.
- **Children index:** a `Children(SmallVec<Entity>)` component, kept in
  sibling order and maintained by `reparent`, `reorder_sibling`,
  `move_sibling_*`, `spawn_object*` and `despawn_tree`. This is the layout
  Unity, Godot and Bevy use. `children_of` becomes O(k).
- Both indexes must stay correct through undo and redo, snapshot restore,
  replication apply and PIE world copies. Any path that writes `Parent` or
  `StableId` directly bypasses them, so either make every write go through
  one place, or rebuild the index from the journal when it reports
  `Overflowed`.
- Add a debug-only consistency check (index vs. a full scan) to the existing
  `world_store_regressions` tests.

Done when: `entity_for` and `children_of` no longer scan, and
`subtree_ids`, `get_all_objects`, the `classes.rs` helpers, `history.rs`,
`reconcile.rs` and the others get faster without changes of their own.

## Wave 4: a persistent, incrementally patched outliner model

Replace "rebuild every row from the world" with a `HierarchyModel` owned by
the panel:
- Rows are stored by `Entity` (`HashMap<Entity, Row>`, plus ordered children
  from the wave 3 index), with ids as `Arc<str>`, so there are no `String`
  clones.
- Each frame, drain the change cursors from wave 2. For each changed entity,
  patch only that row: rename, reparent (move it between two child lists),
  add, remove, change visibility, change icon.
- On `ChangeRead::Overflowed`, or the first time the panel opens, fall back to
  the full O(n) build from wave 0.
- The flattened visible list (expanded rows only) is cached and patched when
  a row is expanded or collapsed or the structure changes, not rebuilt on
  every render.
- Hand the widget `Rc` slices instead of cloning `cached_items`.

This works the same way as Unreal's pending-operations queue. If an edit
touches a very large number of objects at once (a paste, a level load), the
patching can be spread over a fixed number of operations per frame.

Done when: editing one object costs O(1) in the outliner, and a 100k-object
level stays interactive while scrolling, selecting and renaming.

## Wave 5: only build UI for rows on screen

- `v_virtual_list` already virtualizes painting. Make sure no per-row work
  happens before it: no building `SceneObjectItem` for every row, no
  per-row closures for rows off screen. Build the row's item lazily inside
  the virtual list's range callback from the model in wave 4.
- Compute the class role, movability and icon lazily for visible rows, and
  memoize them per entity, invalidated through the journal.
- Search and filter run on the model with a cached match set and are
  re-evaluated only for changed rows.

## Order and size

| Wave | Size | Depends on | Payoff |
|---|---|---|---|
| 1 widget quadratics | S | none | Large on flat levels; every tree view benefits |
| 2 selection ≠ structure | M | none | Removes the rebuild on click entirely |
| 3 scene indexes | M-L (crosses SceneDB) | none | Fixes about 228 scan sites engine-wide |
| 4 incremental model | L | 2, 3 | O(changed) for every edit |
| 5 lazy rows | M | 4 | O(visible) render |

Waves 1 and 2 are independent, can run in parallel, and remove the selection
stall completely. Wave 3 is the foundation for everything else and fixes
editor-wide stalls far beyond the hierarchy. Waves 4 and 5 make the outliner
scale with what changed and what is on screen.

## How to measure

- Use a stress level of 10k / 50k / 100k objects in two shapes: flat (all
  roots, the worst case for wave 1) and deep or nested (folders).
- Record the existing `render_stats` scopes (`hierarchy: refresh cache`,
  `hierarchy: ECS snapshot`, `hierarchy panel: element build`) for a
  selection click, a rename, a reparent, and scrolling.
- Targets: a selection click under 1 ms in the hierarchy at 100k objects;
  editing one object O(1); scrolling O(visible).
