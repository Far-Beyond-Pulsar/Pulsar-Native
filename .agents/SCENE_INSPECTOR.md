# SceneDB Inspector

`crates/editor/ui_scene_inspector` (tool layer): a window (Tools menu and
status bar, "SceneDB Inspector") over a live `pulsar_scenedb::World`. It only
reads; it has no way to edit a scene.

## Which scenes

`engine_backend::services::scene_directory`: a scene's owner calls
`publish(&shared_scene, label)` and keeps the returned `PublishedScene`. The
directory holds weak handles only. The level editor panel publishes its scene
(Play-in-Editor shares that world, so a running game is inspected there).
Anything else with a `SharedScene` (a preview editor, a plugin's editor) is
listed by publishing it the same way. A game in its own process is not listed.

## What it shows

| Pane | Content |
|---|---|
| Browser | **Objects** (the object tree), **Entities** (all, by index), **Types** (component types, each opening to its entities), **Archetypes**. Search: words match name, stable id and `12v1`; `has:Type` / `c:Type` by component; `-term` excludes. Arrows, Page Up/Down, Home/End move; left/right close and open |
| Components | The entity's identity (index, generation, bits, archetype, row, stable id, owner), its components, then those of each attached component instance |
| Data | **Fields**, **Hex**, **Binary** |

Entity references in fields, owners and column gutters are links; Back and
Forward walk the selection history. "Follow editor selection" tracks the level
editor's primary selection. Selecting another entity keeps the same component
type shown.

## Fields

`utils::detail::decode`, first that applies:

1. the scene model's own types (`utils::builtins`: they register no
   reflection; `UnresolvedComponent` shows its stored payload and reason);
2. a registered component class's `#[property]` getters, with categories;
3. `RUNTIME_TYPE_REGISTRY` for the type (declaration order). If one member has
   no serializer the whole struct cannot serialize: every field is still named,
   with the error;
4. nothing: only the bytes are offered.

A new scene model component should get an arm in `builtins`. A plugin needs
nothing: classes and `Reflectable` types are already covered.

Values are JSON next to their `RuntimeTypeInfo`; `utils::fields::rows` builds
rows only for what is open. Lists over 100 open as ranges. Enums, bools,
colors (a name or type containing "color" with 3 or 4 numbers), 2 to 4 number
vectors and entities have their own widgets. Changed rows and bytes are
highlighted for 1.5 s.

## Bytes

The value as it lies in its column (`size_of_val` of `World::get_dyn`), so heap
data appears only as pointer, length and capacity. "This entity" or "All
entities (column)": each record starts a row, read a page (at most 65,536
rows) at a time. Bytes are tinted by field where the layout is known
(`RuntimeTypeInfo` struct offsets, or `offset_of!` in `builtins`) and named in
the legend. The footer reads the bytes at the cursor as numbers, either
endianness. Click, drag, shift-click, arrows, Ctrl+A, Ctrl+C; the offset box
takes `31`, `0x1f` or `1fh` and turns pages.

## Reading the scene (`handlers::refresh`)

- The UI thread never takes the scene's lock. One job at a time runs on the
  background executor; its result is applied on the UI thread.
- The index scan locks in slices of 2,048 entities, because the renderer
  writes the scene every frame. It is skipped while `World::revision` stands.
- While Live, the value is read at most every 200 ms and the index every
  750 ms, and each less often the longer its last read took (20x and 25x), so a
  huge scene or value cannot crowd out writers. The status line shows both costs.
- A component's value is serialized under one read lock. A very large one
  holds writers for that long; the back-off bounds how often, and Pause stops it.

Every list (browser, components, fields, bytes, legend) is a `uniform_list`.

## Tests

`cargo test -p ui_scene_inspector`: the model on plain worlds, and one headless
window test over 40,000 objects that draws each view and tab, live and paused
refresh, column paging, the byte grid's keyboard, a long list, history, despawn
and the scene closing. It checks state, not pixels.

## Not covered

- GPU mirror buffers (row buffers, var-len pools) are not read back. The VRAM
  Map shows what they take (`VRAM.md`).
- Editing values. It would have to go through the level editor's commands to
  be undoable.
- `crates/external/scenedb/crates/scenedb_inspector` is SceneDB's separate
  egui client over a shared-memory bridge; the editor does not use it.
