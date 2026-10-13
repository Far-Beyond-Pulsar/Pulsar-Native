> **Status: implemented** (branch `layout-persistence`). As built: see `.agents/LAYOUT.md`.
> Deviations that were decided while building:
>
> - Mission Control, Git Manager, Problems and the Type Debugger are separate OS
>   windows, not dock chrome; they are **not persisted** (only the file manager
>   drawer's docked / height are).
> - Pop-outs **do not claim live singleton panels**; their tabs are rebuilt from
>   restore keys and files only. Dormant tabs inside pop-outs are supported
>   (`DormantRoot::Popout(id)`, empty "ghost" pop-outs).
> - **No secondary editor window of the same project exists yet**: slots other than
>   0 are implemented and unit tested but nothing creates a second window.
> - A restored maximized pop-out on a vanished display comes back windowed.
> - The `PanelWindow` / `create_detached_window` pop-out path was deleted.

# Layout persistence, complete (Pulsar-Native#1016)

Goal: every piece of editor layout the user can arrange survives a restart,
through the seams the architecture already has. No special cases for particular
panels in the persistence code.

## What the code looks like today (verified)

- `ui_core/src/app/layout_persistence.rs` (≈890 lines): save (debounced, on
  `DockEvent::LayoutChanged` + window bounds + close), restore (`Restorer`
  builds `DockItem`s *while mutating* the live panel pool and the anchor
  `TabPanel`), `SavedLayout { version, dock, window, sidebar }`.
- `ui` = `crates/ui/wgpui-component` (submodule). `DockAreaState` has
  `center` + three `DockState`s. `PanelState.info = Panel(json)`; the only
  identity convention is `{ "file": path }` (`with_file` / `file` / `file_mut`).
- **Pop-outs are dock-native**: `dock/popout.rs::open_popout_window` builds a
  *separate `DockArea`* ("detached-dock") in its own window, decorated by
  `ui_common::popout_decorations`. Its `PanelSource` (where to return on
  close) is runtime-only. `DockWindowRegistry` (`tab_drag.rs`) already tracks
  every live dock window + channel.
- `ui_core` `PanelWindow` / `create_detached_window` / `popped_out_panels`
  have **no callers**: a dead second pop-out path.
- Zoom is owned by `DockArea` (`Zoomed::Group(NodeId)`), `NodeId` is runtime.
- Drawers are loose fields on `AppState` (`drawer_open/docked/height/…`,
  `mission_control_open`, `git_manager_open`) mutated without any event.
- `FileTypeDecoratedPanelView` (plugin_manager) forwards `dump()`/
  `panel_file_path()`, so plugin editors already reach persistence only
  through those two methods.
- Sidebar persistence (`nav_sidebar::model::SavedSidebar`, `save(&root)` /
  `restore(saved,&root)`) is the precedent for "a subsystem owns its saved
  shape".

## Design

### 1. Restore is plan-then-commit (fixes "no rollback", "not transactional")

Replace `Restorer` mutating as it goes with two phases:

1. **Resolve (no side effects on the live UI).** Walk the saved tree and
   produce a pure `Plan` (`Split{axis,sizes,children}` / `Tabs{panels,active}`
   / `Tiles` / `Popout`). Panels are *resolved* (live singleton claimed by
   name, restorer-created, or file-opened) but nothing is attached; the live
   pool is a read-only lookup, claims are tracked in the resolver.
2. **Validate**: centre contains the anchor's panels (level editor); every
   claim unique; zoom target exists. A failure discards the whole plan; the
   live layout was never touched.
3. **Commit**: attach in one `update` pass (fill anchor, `set_center`,
   side docks, tiles, pop-outs, zoom, chrome). Commit steps are entity
   updates with no failure path left.

This supersedes the pool-snapshot patch in this branch, which can be reverted
once phase 1 lands. The resolver is pure data, so collapse / pruning /
anchor-claim logic is unit-testable without GPUI later.

### 2. Panel identity: a restorer registry (items 2, 7)

Generalise `{ "file": … }` to a **restore key** carried in the same
`PanelInfo::Panel(json)` slot (additive, old files keep working):

```json
{ "file": "…" }                                  // existing
{ "restore": { "kind": "git.diff", "data": {…} } } // new
```

- `PanelState::with_restore(kind, data)` / `restore_key()` in the dock crate.
- A global `PanelRestorers` registry (lives in `plugin_editor_api`, host side
  in `plugin_manager`): `kind -> fn(data, window, cx) -> Option<Arc<dyn
  PanelView>>`. Plugins register theirs through the same SDK path as editors;
  `ui_core` registers its own (agent chat, manual tool, diff view) the same
  way; built-in singletons stop being special-cased by name.
- Resolution order in the resolver: live singleton by `panel_name` → restorer
  by `kind` → file via `create_editor_for_file`.
- **Ephemeral** panels declare it (`PanelState::ephemeral()`), so skipping is a
  decision, not silent loss.
- **Unresolvable but identified** (plugin disabled or missing): keep the leaf
  in a `dormant` list written back verbatim on the next save, so disabling a
  plugin for one session does not erase its tabs (mirrors `InvalidPanel`
  "carries the original PanelState forward" in the dock).
- Per-editor view state (item 5) rides in `data` later; no format change.

Open dependency: confirm how a plugin editor exposes `dump` across the
permanent-DLL ABI (`PLUGIN_SYSTEM.md`) and add `restore(state_json)` next to
`create_editor` in the same style.

### 3. Zoom (item 3)

Additive `DockAreaState.zoomed: Option<ZoomedState>` (`serde(default)`, skip
when `None`). `ZoomedState` names the zoomed *group by the identity of its
active panel* (restore key / file / name) plus its tree path as fallback, never
by `NodeId`, so it survives the pruning/collapse the restore does.
`DockArea::dump` fills it from `Zoomed::Group`; commit applies it with the
existing `set_zoomed_in` after the trees are installed. A zoom whose group
vanished is dropped.

### 4. Chrome / drawers (item 4)

- Introduce `ChromeState` (`ui_core/src/app/chrome.rs`) replacing the loose
  drawer fields: `file_manager { docked, height }`, `mission_control_open`,
  `problems` (docked/height if it is dockable), `git_manager_open`. Transient
  overlay state (hover-open drawer, resize drag, drag-suppress) stays
  non-persisted fields.
- `ChromeState::save()` / `restore()` following `SavedSidebar`.
- Every mutation goes through `PulsarApp::chrome_changed(cx)` which calls
  `schedule_layout_save` (these paths emit no `DockEvent` today, so currently
  nothing would arm the save).

### 5. Pop-outs (item 1)

Dock crate (submodule, additive):
- `popout::live_popouts(cx) -> Vec<PopoutInfo { window, dock_area, parent }>`
  and a `PopoutEvent::{Opened,Closed,Changed}` global event, built on
  `DockWindowRegistry`, so the app persists them without reaching into private
  state. `Changed` forwards the pop-out dock's `LayoutChanged` and its
  window-bounds changes.
- Public `open_popout_with(PopoutSpec { tree: DockItem, bounds, source })`
  used by the existing drag-out path and by restore. The drag path becomes
  the single-panel case of the same function.

App:
- Format: `SavedPopout { bounds: SavedWindow, dock: PanelState }`: the whole
  tree, since a pop-out can have been split or had tabs dropped into it.
- `SavedWindow` + display validation move to `layout/window_geometry.rs` and
  are shared by the main window and pop-outs (clamp to a connected display).
- Resolver treats a pop-out as another root; commit opens windows after the
  main layout is in place.
- `source` for a restored pop-out is the centre anchor group, so closing it
  returns its tabs to the main window rather than losing them.
- Delete `PanelWindow`, `create_detached_window`, `popped_out_panels`,
  and `WindowRequest::DetachedPanel` if nothing else uses it.
- Flush order on close: capture pop-outs before the main window tears them
  down (they are still alive in `on_window_should_close`).

### 6. Multiple windows (item 6)

- Format v2 is a map of window slots: `{ version:2, windows:{ "0": {dock,
  window, sidebar, chrome, popouts, dormant}, "1": … } }`. v1 files migrate on
  read as slot 0 (no more "ignore other versions" for 1→2).
- A `LayoutStore` global per project root owns the file: each window saves only
  its slot, via read-modify-write under one lock and the existing atomic
  rename, so windows cannot clobber each other.
- Slot allocation: lowest free ordinal among live windows of that project;
  primary is 0. A window that is closed on its own (others still open) drops
  its slot; closing the last/primary window keeps all slots.
- Startup: primary restores slot 0, then reopens the other recorded slots
  through the existing "new project window" path, each restoring its own slot.
- To verify before building: what a secondary window's centre contains (does
  it have its own level editor / anchor). The resolver's anchor rule must be
  parameterised if not.

### 7. Module layout

Split the file along UI_CRATES conventions:

```
ui_core/src/app/layout/
  mod.rs            wiring: init, schedule/save_now, window events
  format.rs         SavedLayout v2, migration, read/write (atomic)
  window_geometry.rs
  resolve.rs        Plan + Resolver (pure)
  commit.rs         Plan -> live UI
  chrome.rs         ChromeState save/restore
  store.rs          LayoutStore, slots
```

## Order of work (submodule dependency-first)

1. **wgpui-component PR**: `restore` key on `PanelState`, `zoomed` in
   `DockAreaState`, pop-out registry/events/`open_popout_with`. Merge to its
   main, re-pin in Pulsar (`just check-submodule-pins`).
2. Pulsar: split into `layout/`, plan-then-commit, format v2 + migration,
   window_geometry extraction. Revert the pool-snapshot patch.
3. Restorer registry (`plugin_editor_api`, `plugin_manager`, ui_core
   registrations incl. diff view) + dormant list.
4. `ChromeState` and drawer persistence.
5. Zoom.
6. Pop-outs (+ delete dead `PanelWindow` path).
7. Multi-window slots + `LayoutStore`.
8. Docs: new `.agents/LAYOUT.md`, index in `AGENTS.md`; `layout.json`
   gitignore (done for new projects).

Each step is independently shippable and keeps old `layout.json` files
loading.

## Decisions I would like confirmed

1. Closing a restored pop-out returns its tabs to the main window (proposed)
   rather than discarding them.
2. Plugin ABI gets a `restore(state)` entry next to `create_editor`
   (needed for item 2 on plugin panels).
3. Two engine *processes* on one project are last-writer-wins per slot (the
   store merges per slot at write time but takes no cross-process lock).
