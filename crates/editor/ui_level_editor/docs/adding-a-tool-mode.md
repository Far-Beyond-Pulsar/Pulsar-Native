# Adding a Level Editor Tool Mode

Status: current as of Milestone 5 (issue #714, part of epic #709).

This is the practical walkthrough the design doc's §9 open question ("Registration
surface for future external modes") promised: how to add a 6th (7th, …) `ToolMode`
using the Milestone 5 `SplineMode` as the worked example. See
[`level-editor-tool-modes.md`](./level-editor-tool-modes.md) for the full architecture;
this file only covers the mechanics of *adding* a mode.

## 1. Write the mode

A `ToolMode` is a small `struct` implementing the trait in
`level_editor/tool_modes/mod.rs`. Two existing modes are your reference
implementations:

- `tool_modes/level_edit.rs` — `LevelEditMode`, the simplest possible mode (every
  method either does nothing or returns a default).
- `tool_modes/terrain/mod.rs` — `TerrainMode`, the complex end of the spectrum
  (brush strokes, the `TerrainEditApi` seam, undo coalescing).
- `tool_modes/spline.rs` — `SplineMode` (Milestone 5), a minimal-but-real mode
  in between: real per-frame state, no renderer seam.

Trait methods to implement (see `ToolMode` in `tool_modes/mod.rs` for exact
signatures):

| Method | Required? | Purpose |
|---|---|---|
| `id`, `label_key`, `icon`, `description_key` | Yes | Identity — dropdown entry, tooltip, mode indicator. |
| `on_pointer` | Yes | Viewport click/drag/hover routing. Return `Consumed` to suppress the default pick/gizmo path, `PassThrough` to allow it. |
| `clone_box` | Yes | `Box::new(self.clone())` (or `Box::new(*self)` for a `Copy` mode) — needed because `ToolModeRegistry` is `Clone` and the modes are trait objects. |
| `on_mode_entered` / `on_mode_exited` | Has a default (no-op) | Override to reset transient state or close an in-progress gesture when switching away. |
| `toolbar_controls` | Has a default (empty) | Declarative widgets (`ToolWidget::{Slider,Segmented,Toggle,Action,Divider}`) the toolbar renders while this mode is active. |
| `status` | Has a default (`None`) | One-line status bar readout + tooltip. |
| `brush_cursor` | Has a default (`None`) | **Currently dead code** — see the note at the end of this doc before you rely on it. |

## 2. Decide where the mode's state lives

This is the one design decision every prior milestone had to make, and the
rule is simple:

- **A field on the mode struct itself** when only the mode's own methods ever
  need it — i.e. it's read and written entirely from within `on_pointer`,
  `on_mode_entered`/`_exited`, and friends, called through the `&dyn ToolMode`
  the registry hands out. Precedent: `TerrainMode::last_stamp_center_m` (drag
  coalescing state nobody outside `TerrainMode` touches).

- **A new sub-struct on `EditorDomain`** (a "domain", next to `TerrainDomain`)
  when something *outside* the mode's own trait-object needs to read (or
  write) it. In practice that means: `toolbar_controls`/`status` build their
  own throwaway `ToolModeContext` from `&LevelEditorState` in
  `ui/toolbar/tool_mode_controls.rs` and `ui/status_bar_view.rs` — they call
  through `state.editor.tool_mode_registry.selected()`, i.e. through the
  trait object, so that part alone doesn't force a domain. What *does* force
  one is `ToolbarSignature`/`StatusBarSignature` (see §3): those are built
  directly from `&LevelEditorState` fields, with no way to reach into an
  opaque `Box<dyn ToolMode>`. If your `toolbar_controls`/`status` output
  depends on state that changes over time, that state needs a concrete,
  named field the signature structs can read — hence a domain. Precedent:
  `TerrainDomain` (brush settings, read by the toolbar); Milestone 5's
  `SplineDomain` (point list, read by both the toolbar and the status bar).

  A domain is also required for anything that must survive a mode switch —
  fields on the mode struct are never cleared when you switch away (they're
  just not being read), but if the *design intent* is persistence, a domain
  makes that explicit and matches how `TerrainDomain`'s brush settings work.

Either way, wire a new domain in with **one line** in `EditorDomain`'s struct
body and its `Default` impl (`state/editor.rs`) — see how `spline:
super::spline::SplineDomain` was added for Milestone 5. That is the only
Milestone 1-4 file this pattern touches, and only additively.

## 3. The `ToolbarSignature`/`StatusBarSignature` gotcha

`ToolbarView` (`ui/toolbar/view.rs`) and `StatusBarView`
(`ui/status_bar_view.rs`) are cached GPUI views: they only re-render when a
`PartialEq` snapshot of "everything they read" changes. That snapshot is a
plain struct (`ToolbarSignature`, `StatusBarSignature`) built by hand from
named `LevelEditorState` fields — it has no way to generically notice that an
opaque `Box<dyn ToolMode>` changed internally.

**If your `toolbar_controls` or `status` output depends on state that
changes at runtime, add that state to both signature structs and their
`::of()` constructors, or the toolbar/status bar will render stale** — the
widgets will show last frame's numbers until something *else* happens to
invalidate the cache. This has bitten every prior milestone in some form
(`view.rs` has carried an explicit warning comment about it since Milestone
1). Milestone 5 added `spline_point_count`/`spline_length_m` to both
structs when `SplineMode`'s toolbar/status readouts started depending on
`EditorDomain::spline`.

## 4. Register it

Do **not** add a third entry to `ToolModeRegistry::builtin()` — that
function is reserved for the two shipped defaults (`LevelEditMode`,
`TerrainMode`) and Milestone 5 proved it doesn't need to grow. Instead,
add your mode inside `register_tool_modes` in `tool_modes/registry.rs`:

```rust
pub fn register_tool_modes(registry: &mut ToolModeRegistry) {
    registry.register(Box::new(super::spline::SplineMode::default()));
    // registry.register(Box::new(super::your_mode::YourMode::default()));
}
```

`register_tool_modes` is called once, from `EditorDomain::default()`
(`state/editor.rs`), immediately after `ToolModeRegistry::builtin()`. That
call site is the *only* place outside your mode's own files this pattern
touches:

```rust
let mut tool_mode_registry = ToolModeRegistry::builtin();
register_tool_modes(&mut tool_mode_registry);
```

Once registered, the mode dropdown (`ui/toolbar/tool_mode_dropdown.rs`), the
mode indicator (`ui/toolbar/mode_indicator.rs`), the toolbar's widget
renderer (`ui/toolbar/tool_mode_controls.rs`), the status bar
(`ui/status_bar_view.rs`), and viewport pointer dispatch
(`ui/viewport/mod.rs` → `ToolModeDispatcher::dispatch_pointer`) all pick it
up automatically — every one of those already iterates
`registry.modes()`/calls through `registry.selected()` rather than matching
on a fixed set of IDs. You do not need to touch any of them.

## 5. A real limitation: interactive toolbar widgets need a dispatcher edit

`toolbar_controls` can return `ToolWidget::{Slider,Segmented,Toggle,Action}`,
but **clicking one only works if `ToolModeDispatcher::dispatch_widget_edit[_with_terrain]`**
(`tool_modes/dispatcher.rs`) **has a matching arm for that widget's `id`.**
That function is a hardcoded switch over specific ids (`"radius"`,
`PAINT_FOLIAGE_TOGGLE`, `CREATE_FLAT_WORLD`, …) belonging to `TerrainDomain`
— despite the design doc's §4.1 framing of it as "the" generic write-back
seam, it is *not* generic over `ToolMode` implementors. A brand-new mode's
interactive widgets are inert until a maintainer adds an arm there.

Milestone 5 was built under a hard constraint of zero edits to
`ToolModeDispatcher`, so `SplineMode` never hits this: every real mutation
it makes goes through `on_pointer` (which *is* fully generic — the
dispatcher calls it through the trait object with no per-mode branching),
and its toolbar widgets are read-only-by-construction (`Slider` with
`min == max == value`, which `tool_mode_controls.rs` already renders with
both increment/decrement buttons disabled). If your mode genuinely needs a
clickable toolbar control, budget for a small, purely additive
`ToolWidgetEdit` arm in `dispatch_widget_edit[_with_terrain]` — that edit
does not touch `LevelEditMode`'s or `TerrainMode`'s own arms, but it is a
real, necessary exception to "only touch your own files," and the actual
generic fix (routing unhandled widget ids through the active mode itself)
is left for whoever needs it next.

## 6. `brush_cursor` is currently unused

`ToolMode::brush_cursor()` is never called anywhere in the shell — the
in-viewport brush ring is instead drawn by `TerrainMode` pushing directly to
the renderer's mailbox (`TerrainEditApi::set_brush_cursor`), bypassing the
trait method entirely. Overriding `brush_cursor` costs nothing and may be
useful as a hook for a future generic renderer, but as of Milestone 5 it has
no effect on what's drawn — don't rely on it for actual on-screen feedback
without first wiring a caller for it.
