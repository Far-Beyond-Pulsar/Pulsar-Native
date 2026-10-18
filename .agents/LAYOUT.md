# Layout persistence

How the editor remembers what the user arranged: dock tree, tabs, zoom, drawers,
window geometry, pop-outs, and every project window. Code:
`crates/editor/ui_core/src/app/layout/` (+ `app/chrome.rs`). Dock-side pieces
(`PanelState` restore key / ephemeral, `DockAreaState.zoomed`, pop-out registry
and events) live in the `ui` crate (`crates/ui/wgpui-component`, submodule).

Plan and rationale: `.agents/plans/layout-persistence/PLAN.md`.

## File

`<project>/.pulsar/layout.json`, per user, written atomically (write beside,
rename). Format version 2 is a map of window **slots**:

```json
{
  "version": 2,
  "windows": {
    "0": {
      "dock": {
        "center": { "panel_name": "StackPanel", "children": [ "...tab groups..." ], "info": { "...": "..." } },
        "left_dock": { "panel": "...", "placement": "left", "size": 280.0, "open": true },
        "zoomed": { "panel": { "panel_name": "Editor", "info": { "panel": { "file": "Content/Hero.class" } } },
                    "placement": "center", "path": [0, 1] }
      },
      "window": { "state": "Maximized", "bounds": { "origin": {"x": 120.0, "y": 80.0}, "size": {"width": 1500.0, "height": 900.0} } },
      "sidebar": { "pinned": [], "groups": [], "collapsed_groups": [], "expanded_folders": [] },
      "chrome": { "file_manager_docked": true, "file_manager_height": 512.0 },
      "popouts": [
        { "id": 0, "bounds": { "state": "Windowed", "bounds": { "...": "..." } },
          "dock": { "panel_name": "TabPanel", "children": [ "..." ], "info": { "...": "..." } },
          "zoomed": null }
      ],
      "dormant": [
        { "root": "center", "anchor": { "file": "Content/Hero.class" },
          "state": { "panel_name": "Editor", "info": { "panel": { "restore": { "kind": "acme.preview", "data": {} } } } } },
        { "root": { "popout": 0 }, "state": { "...": "..." } }
      ]
    },
    "1": { "...": "a second project window" }
  }
}
```

(`...` stands for the dock crate's own serialized shapes; open a real file for
exact output.) Empty / default parts are omitted: `chrome`, `sidebar`, `zoomed`,
`popouts`, `dormant`, `window`.

- **Paths.** File paths inside the project are stored project-relative (forward
  slashes) and made absolute on load. This covers the tree, `zoomed`, pop-out
  trees and their `zoomed`, and dormant leaves and their anchors
  (`SavedWindowLayout::relativize` / `absolutize`). Restore `data` is opaque and
  never rewritten.
- **v1 -> v2.** A version 1 file (`{ version: 1, dock, window, sidebar }`) is read
  as slot `"0"`; the next save writes version 2.
- **Unknown data survives.** Slots are kept as raw JSON when another slot is
  written, so a slot owned by a window or a later version is carried over
  verbatim. A file of a version this build does not know is **never
  overwritten** (the save fails with a warning); a file that is not valid JSON
  or has no usable `windows` map is replaced after a copy is kept at
  `layout.json.bak`; a slot that does not parse is copied there too before the
  next save replaces it.
- **Lenient lists.** `popouts` and `dormant` entries that do not parse are skipped
  one by one rather than making the slot unreadable.
- **Bounds on load.** Restoring opens at most 16 pop-outs, re-offers at most 64
  dormant tabs, and the startup reopen of other windows is capped at 8 slots.

## Slots and windows

Every editor window of a project owns one slot (`store.rs`, `LayoutStore`, a
process-wide singleton). A window claims the lowest ordinal not held by a live
window of the project when it is built, so the primary window is slot `0`. Each
window reads and writes only its own slot; writes are read-modify-write under one
in-process lock plus the atomic rename. Two engine **processes** on one project
are last-writer-wins per slot (no cross-process lock).

- Closing a secondary window on its own while others remain drops its slot.
  Closing the primary, the last window, or anything once the app is quitting
  (`on_app_quit` marks it before windows are torn down) keeps all slots. The slot
  is released when the window entity is released, so a vetoed close loses nothing.
- At startup the primary window reopens the other recorded slots once per process
  through `PulsarRoot::open_additional`, each restoring its own slot and
  geometry.
- No feature creates a second editor window of the same project yet, so only slot
  `0` occurs today; the machinery is in place and unit tested but never run with
  two windows.
- A window without a level-editor tab (empty anchor group) accepts any centre.

## Save

`schedule_layout_save` arms a 4 s debounce; each further change re-arms it.
Triggers: `DockEvent::LayoutChanged` (tabs, splits, resize, dock toggles, zoom),
window moved/resized, pop-out events, and `PulsarApp::chrome_changed`.

Gating, all in `schedule_layout_save` / `save_layout_now`:

- `layout_persist` - the window has a project.
- `layout_ready` - set only after the restore finished (or there was nothing to
  restore). Nothing is written before it, so the default layout can never
  overwrite a saved one, including a window closed before its restore ran.
- `layout_closing` - set after the final save in `on_window_should_close` and again
  when the entity is released. Pop-outs closing along with the window then cannot
  save a layout without themselves.

The final save runs in `on_window_should_close`, while pop-outs are still alive.

## Restore: resolve, validate, commit

Runs once, deferred until the window and plugin manager are up
(`restore_layout`).

1. **Resolve** (`resolve.rs`, no side effects on the live UI). Each root (centre,
   left / right / bottom dock, every pop-out) is walked into a pure `Plan`
   (`Split` / `Tabs` / `Tiles`). The tree logic (`resolve_tree`) is generic over
   a `LeafResolver` and unit tested with fakes: leaves that cannot be resolved are
   pruned, emptied groups dropped, a split left with one child collapses (its
   saved sizes are discarded when a side vanished).
   A saved tab resolves in this order (`resolve_in_order`):
   1. `ephemeral` -> skipped;
   2. a live singleton with the same `panel_name` (agent chat, level editor, ...),
      claimed by entity id (main-window roots only; pop-outs never claim);
   3. a restorer registered for its restore `kind`;
   4. a file editor through the plugin manager (`create_editor_for_file`).
2. **Validate** (`resolve_layout`). The centre must hold a panel of the anchor
   group (the level editor) when the window has one; no live panel may be in two
   plans. A root that fails is discarded whole and its claims released; the
   live UI was never touched. Live side-dock panels no plan mentions are put
   back where they were.
3. **Commit** (`commit.rs`). One pass attaches everything: the anchor centre
   `TabPanel` is kept and filled in place (several panels and handlers hold that
   entity), side docks are set, zoom is applied, then pop-out windows are opened.
   Chrome and sidebar are applied before resolution (plain data, cannot fail).

## Restorers, ephemeral panels, dormant tabs

A tab is identified by `PanelInfo::Panel(json)` in its `PanelState`:

```json
{ "file": "Content/Hero.class" }                              // file editor
{ "restore": { "kind": "acme.preview", "data": { "x": 1 } } } // restorer
```

Both can coexist; a restorer wins and a restorer that declines is final.

**Making a new panel restorable**

1. Return `PanelState::new(self).with_restore("<crate>.<panel>", data)` from the
   panel's `dump()` (`data` is whatever the restorer needs; keep it small and
   stable; per-editor view state can ride here).
2. Register `kind -> fn(&Value, &mut Window, &mut App) -> Option<Arc<dyn PanelView>>`:
   - plugins: `registry.register_restorer(kind, ...)` inside `register_editors`
     (see `plugin_editor_api::panel_restore` and `PLUGIN_SYSTEM.md`, "Panel
     restorers"; no new ABI symbol);
   - built-in code: `plugin_manager::panel_restorers().register(kind, ...)`.
3. Return `None` when the data no longer makes sense (declined: the tab is dropped).

A panel that opens a file only needs `panel_file_path()` (already forwarded by
the file-type decorator); no restorer is needed.

A live singleton the app creates itself needs nothing: it is claimed by
`panel_name`. Keep its name stable.

**Making a panel ephemeral.** Return `PanelState::new(self).ephemeral()` from
`dump()` for a panel that must not come back (a transient preview, a progress
view). Skipping is then a decision with a debug log, not silent loss.

Note: as of this writing no built-in panel has a restore key yet; the registry is
in place and tested, and the first users will be plugins (and later the diff view
and similar built-ins).

**Dormant tabs.** An identified tab that cannot be built *because nothing is
registered* (unknown restore kind, no editor for the file type: the plugin is
disabled or missing) is not dropped. It is kept as a `DormantLeaf { root, anchor,
state }`, written back under `dormant` on every save (pruned: bounded to 64,
deduplicated, and file tabs whose file vanished removed) and re-offered to the
resolver on the next load, where it resolves normally if the plugin is back.
`root` says which tree it belongs to (`center`, `left`, `right`, `bottom`, or
`{ "popout": id }`); `anchor` is the identity of a sibling tab so it returns to the
same group. A tab whose file is missing, whose restorer declined, whose editor
failed, or that has no identity is gone, not dormant.

A pop-out is a root of its own, named by its stable `id` (positions shift when one
closes). A pop-out whose tabs are *all* dormant opens no window and stays in the
file as an empty pop-out so its tabs have somewhere to return; it disappears once
no dormant tab refers to it. Closing a pop-out discards its dormant tabs.

## Zoom

`DockAreaState.zoomed` names the zoomed tab group by the identity of its active
panel (file / restore key / name) plus its dock placement and tree path as a
fallback, never by a runtime `NodeId`. `DockArea::dump` fills it,
`DockArea::apply_zoomed` finds the group again and zooms it. Commit applies it
after the trees exist; a zoom matching nothing is dropped. Pop-outs have their
own dock and their own `zoomed` field, applied once their window is open.
`set_zoomed_in/out` emit `LayoutChanged`, so zooming arms the save.

## Chrome

`ChromeState` (`app/chrome.rs`) owns the former loose drawer fields. Saved
(`chrome`): file manager docked / not, the folder it shows (project-relative, restored only if it still exists), and its height (clamped on load; non-finite
values fall back to the default). Every mutation of saved chrome must call
`PulsarApp::chrome_changed(cx)`, because those paths emit no `DockEvent`.

## Pop-outs

`ui::dock::live_popouts` lists the open pop-out windows (each with its own
`DockArea` and parent window) and `ui::dock::popout_events` emits
`Opened / Changed / Closed`. The events fire inside window callbacks, so
`Opened` / `Changed` are handled on a later turn (`on_popout_event`), and events of
pop-outs owned by another window are ignored. A save stores each live pop-out as
`{ id, bounds, dock, zoomed }`: geometry (a maximized window stores the windowed
size to return to) and the whole tree. On restore, windows open last through
`open_popout_with`, with bounds checked against the connected displays
(`window_geometry::fit_to_displays`). A pop-out on a vanished display opens
windowed at the default size near the primary display's corner; if it had been
maximized it therefore comes back windowed rather than maximized. Closing a
restored pop-out returns its tabs to the main window's centre anchor group.

## Deliberately not persisted

- **Per-editor state** (cursor, scroll, selection inside an editor): that belongs
  to the editor, not the layout. A panel that wants it puts it in its restore
  `data`.
- **The overlay drawer** (`drawer_open`): a momentary peek; restoring it would
  greet the user with an open overlay. Also resize-drag and drag-suppress flags.
- **Mission Control, Git Manager, Problems, Type Debugger**: they are separate OS
  windows opened through the window registry, not part of the dock. Their
  status-bar flags only mirror those windows; restoring a flag without the window
  would light a button with nothing behind it (and `git_manager_open` is never
  cleared when its window closes). Reopening such windows at startup is a
  behaviour change this work does not make.
- **Pop-outs claiming singletons**: a pop-out never claims a live singleton panel
  (those sit in the main window's dock and are placed by that root); its tabs are
  rebuilt from restore keys and files only.
- **Ephemeral panels**, by declaration.

## Git

`layout.json` is per user (window geometry, open tabs), so new projects get
`.pulsar/layout.json` in the default `.gitignore` written by
`engine_backend` (`subsystems/networking/git_sync.rs`). Existing projects that
already track it should add the line themselves; `layout.json.bak` can be ignored
the same way.

## Not exercised at runtime

The pure parts (format, migration, slots, resolve tree logic, dormant, window
geometry, chrome) are unit tested. The GPUI-facing parts (restorer-built panels in
a live window, commit, zoom application, pop-out open / events / capture,
multi-window slots, reopen at startup) were compile-checked only; see the
branch's final report.
