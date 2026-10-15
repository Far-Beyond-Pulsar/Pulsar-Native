# Flamegraph viewer: session (file) mode overhaul

Scope: `crates/editor/ui_flamegraph`, the screen shown once a session is loaded
(or recorded) and `has_data` is true. The start screen and recording overlay
are out of scope except where they hand over to this mode.

## What it is today

`FlamegraphWindow::render` (`window.rs`) shows, for a loaded session:

- a title bar, then a row of read-only **summary chips** (state, Spans, Threads,
  Frames, Duration, file name);
- three fixed **horizontal resizable columns**: the timeline (800px), the
  statistics panel (400px) and the scripts panel (400px). Sizes are not saved,
  nothing collapses, and the timeline gets squeezed to make room for two tables.

`FlamegraphView` (`flamegraph_view.rs`, GPU timeline) has: wheel zoom, drag pan,
crop-drag to zoom into a range, hover popup, double-click to select a span (draws
cross-thread dependency arrows), frame-time graph on top, ruler, fixed 120px
thread labels.

`StatisticsPanel` aggregates by name only (calls, total, avg, min, max), renders
the first 100 rows as plain children (not virtualized), no search, no self time,
and is not connected to the timeline. `ScriptsPanel` only understands
`script:`-prefixed spans, so it is empty for most sessions yet takes 400px.

### Gaps that hurt most

1. Layout is fixed and wasteful; no way to give the timeline the full window, or
   to dock analysis below it.
2. No toolbar: no open / save / export, no fit / zoom-to-selection, no frame
   stepping, no toggles for frame lines, arrows or labels.
3. No navigation aids: no minimap, no vertical scrollbar, no keyboard, no
   back/forward through view history.
4. Threads cannot be collapsed, hidden, pinned, searched or resized.
5. Analysis is shallow and disconnected: no self time, no call tree, no
   bottom-up, no percentiles, nothing restricted to a selected range, and
   clicking a row does not show where the function ran.
6. Selection is a single double-clicked span; hover popups are not pinnable and
   cannot jump to source.

## Target layout

```
┌ Toolbar: File ▾ | Fit  ⊕ ⊖  ◀ Frame 214/980 ▶ | Search [____] | View ▾ | Layout ▾ ┐
├ Overview strip (minimap + frame-time graph, draggable viewport window) ─────────────┤
├ Timeline (thread headers │ GPU canvas, vertical scrollbar) ──────────────┬ Inspector ┤
│                                                                          │ (selected │
├ Bottom dock, tabbed: Statistics | Call Tree | Bottom-Up | Frames |       │  span /   │
│   Scripts | Markers | Search results                                     │  range)   │
├ Status bar: cursor time │ selection length │ zoom │ threads shown/total ─┴───────────┤
```

- Built on the existing dock/resizable primitives (`ui::dock`, `resizable`), so
  every region collapses, resizes, maximizes (double-click tab) and is persisted
  per project in `.pulsar/layout.json` (see `.agents/LAYOUT.md`).
- Scripts tab only appears when the session has `script:` spans.
- Summary chips move into the status bar / toolbar tooltip; the file name and
  session metadata live in the title area with a "Close session" action.

## Phases

### Phase 1: layout shell, toolbar, status bar (biggest visible win)

- Replace the three fixed columns with the layout above: timeline fills the
  window; bottom dock and right inspector start collapsed or at sensible
  fractions; persist sizes, open tabs and collapsed state.
- Toolbar actions: open, save as, export (CSV of stats, Chrome trace JSON), close
  session, fit all, zoom to selection, zoom in/out, previous/next frame, toggles
  (frame lines, dependency arrows, labels, minimap, inspector, bottom dock).
- Status bar fed by `ViewState` (cursor time, selection span, zoom, visible
  thread count).
- Introduce `SessionModel` (frame, caches, selection, filters, view history) owned
  by the window and shared with the view and panels via entities/observers,
  replacing the current ad-hoc `TraceData` clones.

### Phase 2: timeline navigation and thread tracks

- Overview strip: whole-session minimap with the visible window as a draggable,
  resizable box; frame-time graph merged into it, click to jump, spikes marked
  (frames over p95/p99).
- Vertical scrollbar and wheel scroll (shift/ctrl modifiers for pan/zoom),
  smooth zoom animation, back/forward view history (alt+left/right).
- Keyboard: W/A/S/D and arrows pan/zoom, `F` fit, `Z` zoom to selection,
  `[` / `]` frames, `Home`/`End`, `Esc` clear selection, `Ctrl+F` search.
- Thread header column: resizable width, collapse a thread to one summary row,
  hide / pin / solo, rename, drag to reorder, filter box, "show only threads
  with spans in view". Group by named threads, pools, process.
- Span coloring modes: by name, by duration heat, by self time, by thread;
  optional labels with elision, min-width switch to merged blocks (existing LOD).
- Range selection (drag on ruler) that stays visible and drives the panels;
  markers/bookmarks placed with `M`, listed in a Markers tab.

### Phase 3: analysis tabs tied to the timeline

- Self time per span (computed once while building `SpanCache`), reused
  everywhere.
- **Statistics**: virtualized (`uniform_list`, no 100-row cap), columns calls,
  total, self, avg, min, max, p50/p95/p99, % of range; group by name / thread /
  category; filter box; scoped to "whole session / visible range / selection".
- **Call Tree** (top-down) and **Bottom-Up** with expand/collapse and the same
  scoping and columns.
- **Frames** tab: frame list with duration, sortable, slowest first; click
  jumps the timeline.
- Row click highlights every instance of that function in the timeline (others
  dimmed) with next/previous occurrence buttons; reverse: selecting a span
  selects its row.
- **Inspector**: pinned span details (name, thread, start, duration, self time,
  parent chain, children, % of frame, source location with "open in editor"),
  multi-select totals, copy as text.
- Compare mode: pick a second session or range and show per-function deltas.

### Phase 4: sessions and files

- Recent sessions list, drag-and-drop of `.db` files, multiple sessions as tabs.
- Background loading with progress for big databases, session metadata (machine,
  build, project, target process).
- Export/import of views (range + filters) as shareable bookmarks.

### Phase 5: quality

- Unit tests: view math (zoom about cursor, fit, frame stepping), self time,
  call tree / bottom-up aggregation, thread filtering, range scoping.
- GPUI visual-context tests for layout restore and keyboard handling
  (existing `VisualTestContext` pattern in `flamegraph_view.rs`).
- Large-trace benchmark (1M+ spans) gating the tile cache and table
  virtualization.
- Document in `.agents/FLAMEGRAPH.md`; add to the `AGENTS.md` index.

## Suggested order

Phase 1, then the virtualized Statistics + self time from Phase 3 (they fix the
worst of today's layout and data problems), then Phase 2, then the rest of
Phase 3. Phases 4 and 5 interleave.

## Decisions needed

1. Docking: reuse `ui::dock` (tabs, drag, maximize, persistence for free) or a
   fixed shell with resizable regions (simpler, less flexible)? Recommended:
   `ui::dock`.
2. Exports in scope for Phase 1: CSV and Chrome trace JSON only, or also
   Speedscope / pprof?
3. Compare mode: needed now, or leave to a later pass?
