# Inspector (properties panels)

One virtualized list for every properties panel. Nothing nests a scroll area;
nothing pages. Content of any size and any row height is windowed by the one
`gpui::list` the panel owns.

| Piece | Where | Job |
|---|---|---|
| `inspector_model` | `ui_common` | Pure model: `CardDesc`/`PropDesc`, search `Query`, `CollapseState`, `flatten` → `InspectorItem`s (`Card`, `Category`, `Property`, `Child`, `Body`). |
| `Inspector<V>` | `ui_common` | Search box, collapse-all, scrollbar, the list, card/category chrome. Host gives it `CardDesc`s and a `render_row` callback. |
| `VirtualRows` | `pulsar_reflection` | An editor offers its content as rows of the panel's list: `row_count`, `render_row`, `on_change(wake)`, `mark_stale`. Carried by `BoundPropertyEditor::rows` / `.with_rows(..)`. |
| `CompositeEditor` | `ui_common::composite_editor` | Fallback editor for any array/map/struct with no editor of its own: a tree over the value's JSON (`composite_model`), only expanded branches become rows. |
| `reflected_list`, `voxel_stack_editor` | `helio-component` | Typed list editors (`Vec<T>` of reflected items, the terrain layer stack), same `VirtualRows` contract. |

## The look

`pulsar_reflection::prims::style` (feature `prims-gpui`, in the reflection
repository) is the one definition of how a reflected value looks, shared by the
property editors, this panel's chrome, `CompositeEditor` and the SceneDB
Inspector: `property_row` (the name in a column `NAME_SHARE` wide, the value
beside it, `ROW_HEIGHT` at least), `control_box` for inputs, and the pieces for
values drawn by kind (`bool_value`, `vector_value`, `color_swatch`,
`variant_box`, `heading`, `chevron`, `mono`).

A new editor lays out with `property_row` (or `prims::editor_row`, which is the
same) and takes its widgets from there; do not hand-roll a label and control
row, or the columns stop lining up. `Inspector::wrap_row` draws the hairlines,
the tint of every second property and the hover.

## Rules

- A property row stays small. Anything that can be big implements `VirtualRows`.
- The host (`ObjectTypeFieldsSection`) reads `PropertyStateManager::virtual_row_counts` each render and calls
  `Inspector::set_extra_rows`; editors wake it through `PropertyStateManager::set_wake`.
- Editors must not do work proportional to their data per frame. Skip `set_value` unless the value's address changed
  or `mark_stale` was called; the host calls `PropertyStateManager::mark_stale(editor_key)` when it re-reads a value.
- Cards and sections start collapsed, and so does everything inside a composite value; field editors are created
  when first drawn. The exception: a composite property with 1 to `OPEN_BY_DEFAULT` (16) children starts open, in a
  host that registered a wake (`PropertyStateManager::set_wake`). It is decided when the editor is built, so edits
  never open or close it.
- A composite list or map with more than `composite_model::CHUNK` (100) children shows as ranges, each opening to
  its items or to narrower ranges. A struct always lists its fields.
- A sub-scroll area is opt-in only (`composite_editor::request_subscroll`), never a shortcut.
- The level editor resolves the project class registry only for a selected class root or generated child, where class variables or slot defaults can appear. Ordinary entities use their attached SceneDB components without scanning project class directories. The selected section retains its registry until the class-definition generation changes.

## Measuring

`cargo test -p ui_level_editor --lib inspector_stress -- --include-ignored --nocapture --test-threads=1`
(1,000,000-leaf array, 200×50 feature spawns, 300-layer stack, 20,000-row card).
Tests that draw a window take `RENDER_LOCK`: `render_stats` is process-global.

Sidebar resize: `cargo test -p ui_level_editor --lib properties_panel_resize_report -- --ignored --nocapture --test-threads=1`
changes the panel width every frame and prints the views rebuilt, the list rows built (`list: item built (visible)` vs
`(measure only)`) and the time per row kind (`properties row: ...`). A resize must cost one draw per step and build
only the rows on screen:

- `gpui::list` keeps off-screen row heights as estimates when its width changes (rows on screen are re-measured every
  frame anyway); it used to discard them all, re-measuring the overdraw above and below the viewport each frame.
- A single-line `InputState` does not notify when only its size changed. That notify, issued during the draw, scheduled
  a second frame that rebuilt every ancestor view.
