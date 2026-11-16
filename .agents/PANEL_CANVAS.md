# Panel Canvas: movable, resizable overlay cards

`ui::panel_canvas` (WGPUI component, `crates/ui/wgpui-component/crates/ui/src/panel_canvas/`)
is the one component set for cards that float over something and can be moved
and resized: the level editor viewport's overlays, and anything like them.
The gallery `crates/ui/wgpui-component/examples/panel_canvas_gallery.rs` shows
every part (run: `cargo run -p ui --features flamegraph --example panel_canvas_gallery`).

## Parts

| Type | What |
|---|---|
| `PanelCanvasState` | Entity the owner keeps (`cx.new`). Slots (add/remove at runtime), each card's placement and size, the drag or resize in flight. |
| `PanelCanvas` | The area element. Styled like a `div`; background transparent unless `.background(..)`; clips overflow (`.clip(false)` to not). `.child` adds fixed, non-moving content under the cards; `.panel(RndPanel)` adds a card. |
| `RndPanel` | A card (resize and drag). Styled `div`, positioned. `.default_slot(id)` / `.default_position(p)`, `.default_size`, `.min_size`/`.max_size`, `.resizable(edges)` to add resize handles. Nests: its content can hold another `PanelCanvas`. |
| `DragHandle` | The shared move handle, anywhere inside a card. Styled `div` with a three-dot grip (`.grip(axis)`, `.no_grip()`). |
| `ResizeHandle` | Sits on one `HandleEdge` (edge or corner) of its card, absolutely positioned. `.grip(true)` draws a corner mark. |
| `PanelRef` | Names one card on one canvas (`PanelRef::new(&state, "id")`); the card and its handles all take it. |
| `Slot` | `id`, `SlotAnchor` (8 edges/corners), `inset` from that anchor, `size`, `SlotFlow` (row/column for several cards), `gap`, `SlotFit`, optional `group`. |
| `SlotFit` | `Expand` (default): the slot grows to contain its cards, away from its anchor or pushing the slots after it along in its group. `Shrink`: the slot keeps its size; its cards are held to it (max size, clipped, sharing it). |
| `SlotGroup` | A strip of slots along one edge (`anchor`, `inset`, `flow`, `gap`), spanning the canvas along its flow and packed towards its anchor in slot order. A growing slot pushes its neighbours along instead of covering them. Slots join with `Slot::group(id)`; their own anchor and inset are then unused. |

A canvas with no slots is a free area: cards go anywhere inside its bounds.

### Full slots

An `Expand` slot whose box has already grown past its `size` along its flow
is full: dragging a card over it lights it in the danger colour with a
not-allowed cursor, and dropping there sends the card back where it came
from. The dragged card is drawn elsewhere during the drag, so it never counts
towards its own slot. Size a slot for what it should hold: the viewport's
toolbar slots are smaller than a toolbar, so each holds exactly one. A
`Shrink` slot is never full.

### Empty slots

An empty slot is not drawn outside a drag, so an empty slot in a group takes
no room. During a drag every empty slot opens as a drop target at the size it
last picked: while holding cards, an `Expand` slot picks its drawn size, never
below its own `size` (`Geometry::picked`). The preview keeps that size until a
card is dropped in, which picks again: back to the slot's own size, or the new
card's if larger (per axis). So the slot a card was just lifted from keeps its
room during the drag and the strip does not jump; slots never filled open at
their `size`. An empty slot is never full, however large its preview.

## Behaviour

- **Drag.** Pressing a `DragHandle` starts a move; the card follows the pointer
  with the grab offset kept, clamped inside the canvas. The slot under the
  pointer lights up (drag-border dashed outline, faint fill). Dropping in a slot
  inserts the card among that slot's cards at the pointer (along the slot's
  flow); elsewhere it stays where dropped, or goes back when the state was
  built with `.allow_free(false)`. A click without movement changes nothing.
- **Resize.** A free card's dragged edges follow the pointer (within min/max and
  the canvas). A slotted card keeps its slot and only its size changes: on the
  slot's free sides the edge follows the pointer exactly; on the anchored side
  it grows outward the other way. Put handles on a slot's free sides.
- **Tracking.** The canvas paints a zero-size `canvas()` that registers
  window-level `MouseMoveEvent`/`MouseUpEvent` listeners (capture phase), so a
  drag or resize keeps going when the pointer leaves the card or canvas; a move
  with no button held ends it (released outside the window). Moves during an
  interaction stop propagation: nothing under the pointer reacts.
- **Placement.** A slot (or a group's strip) hangs from its anchor by absolute
  insets inside a hitbox-free positioner, its box laid out with flex, so no card
  size is needed to place it. Each slot box records where it was drawn
  (`Geometry::slots`, per generation); drops hit that box, joined, for a slot
  outside a group, with its nominal rectangle. Free cards are re-clamped every frame as the canvas
  resizes. Deferred popups (menus, tooltips) escape the clip.

## State and redraws

`PanelCanvasState` is read while rendering, so GPUI re-renders the view that
draws the canvas whenever it notifies (each pointer move during an interaction).
Per-frame geometry (canvas bounds, card bounds, each card's resolved placement
and limits) lives in an `Rc<RefCell<Geometry>>` written from `on_frame` and
render, without notifying; it is what an interaction starts from. Owners set
or query placements with `set_placement`/`placement`, `set_size`/`size`, `reset`.

## Users

- Level editor viewport (`crates/editor/ui_level_editor/src/ui/viewport/overlays.rs`):
  one `PanelCanvasState` per `ViewportPanel` (`overlay_canvas`), slots from
  `overlay_slots()` and groups from `overlay_groups()`: `toolbar-left` and
  `toolbar-right` (one toolbar each), `left` and `right` columns, and the
  `bottom` strip (`bottom-0` .. `bottom-5`, packed right) holding performance,
  pipeline, render stats and Nerds by default, so each shown overlay pushes the
  others left. Toolbars use `floating_toolbar::create_drag_handle`; overlays
  whose content is its own cached view get `floating_toolbar::corner_grip`. The
  warnings card is resizable (right/bottom); its header is the drag handle.
  The loading card, the debug-view legend and the split-view camera sync button
  stay fixed (`PanelCanvas::child`). Placements are per session, not persisted.
