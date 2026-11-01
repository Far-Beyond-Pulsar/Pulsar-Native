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

## Rules

- A property row stays small. Anything that can be big implements `VirtualRows`.
- The host (`ObjectTypeFieldsSection`) reads `PropertyStateManager::virtual_row_counts` each render and calls
  `Inspector::set_extra_rows`; editors wake it through `PropertyStateManager::set_wake`.
- Editors must not do work proportional to their data per frame. Skip `set_value` unless the value's address changed
  or `mark_stale` was called; the host calls `PropertyStateManager::mark_stale(editor_key)` when it re-reads a value.
- Cards, sections and items start collapsed; field editors are created when first opened.
- A sub-scroll area is opt-in only (`composite_editor::request_subscroll`), never a shortcut.

## Measuring

`cargo test -p ui_level_editor --lib inspector_stress -- --include-ignored --nocapture --test-threads=1`
(1,000,000-leaf array, 200×50 feature spawns, 300-layer stack, 20,000-row card).
Tests that draw a window take `RENDER_LOCK`: `render_stats` is process-global.
