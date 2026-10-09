# Unified left sidebar (prototype)

Pulsar-Native#1000. A left sidebar that replaces the horizontal tab bar. It
puts the open editors and the project's folders in one place and leaves the
viewport its full width.

It is off by default. Turn it on with **Settings → Navigation → Unified Left
Sidebar**, or with **Toggle Unified Sidebar** in the command palette.

## What it does

- **Off.** The usual tab strip.
- **Collapsed rail.** The strip is gone and the page fills the editor area.
  A 48 px rail shows one icon per open editor, with groups separated by a
  line and the active editor highlighted. Click an icon to switch to that
  editor.
- **Hover.** Hovering the rail opens the sidebar *over* the editor. The
  viewport does not move or resize. Moving the pointer off the rail and the
  sidebar closes it after 220 ms, so crossing from one to the other never
  flickers. Choosing an editor closes it too.
- **Folder assets.** Choose a folder under **Content** to list its assets in
  the bottom file drawer. In this mode the drawer leaves out its own folder
  tree, since the sidebar has one. Opening an asset opens its editor tab, as
  before.
- **Kept open.** The pin at the top, or **Keep Sidebar Open** in settings,
  puts the sidebar in its own column beside the editor.

The sidebar has two sections:

- **Editors.** Pinned tabs come first, then your own groups, then the rest
  grouped by the kind of editor (every blueprint together, every material
  together), in the order they were opened. Every group collapses.
  - **Rows.** Hover a row to pin or close it. Right-click it to pin it, start
    a new group with it, move it to one of your groups, take it out of its
    group, or close it. A pinned or grouped file stays listed after its tab
    closes, dimmed, and reopens in one click.
  - **Your groups.** Make one with the folder-plus button in the header, or
    from a row's menu; its name is ready to type. Double-click a group's
    header, or use its pencil button, to rename it. Its bin button removes
    the group, and its tabs go back to their editor-kind groups.
  - **Dragging.** Rows and rail icons drag like tabs. Drop one on the editor
    to split it, exactly as with a tab from the strip. Drop one on a group's
    header to move it there: one of your groups, an editor-kind group (which
    takes it out of your group), or Pinned.
- **Content.** The project's `Content` folder, or the project folder when it
  has none. `target/` is not listed. The tree updates as files change on
  disk, because it is the file drawer's own tree.

Pins, your groups (names, tabs, collapsed or not), collapsed groups and
expanded folders are saved per project in `.pulsar/layout.json`, next to the
dock layout.

While the sidebar is over the editor and the file drawer is open, the
sidebar stops at the drawer's top edge, so it never covers the assets.

## How it is built

- `crates/editor/ui_core/src/app/nav_sidebar/`
  - `model.rs`: the ordering, pinning and grouping rules, the folder rows,
    the saved form, and the hover state machine. It is plain data with unit
    tests.
  - `mod.rs`: the connection to the dock. It lists the centre tabs across
    splits, activates, closes and drags tabs, edits groups, applies the
    setting, and lists a folder in the drawer.
  - `render.rs`: the rail, the sidebar, the rows, the menus and the tree.
  - `tests.rs`: an editor window test (see below).
- From the dock (Far-Beyond-Pulsar/WGPUI-Component#24):
  `TabPanel::set_tab_bar_hidden` hides the tab strip, and
  `TabPanel::tab_drag` starts the same drag a tab in the strip starts, which
  is what makes rows draggable into splits.
- The file drawer gained `show_folder`, `set_folder_tree_hidden`,
  `folder_tree` and `selected_folder`.
- The settings are `editor.navigation.unified_sidebar` and
  `editor.navigation.sidebar_pinned`, in `pulsar_settings`.

## Tests

`cargo test -p ui_core --lib -- nav_sidebar` covers both the model and the
editor window test. The window test opens a real editor window (without the
level editor, so it needs no GPU) with four tabs in three editor kinds, and
checks each state above:

- The rail appears and the tab strip goes.
- Hovering opens the sidebar without moving the editor area or the page.
- A folder is listed in the drawer, and the hover sidebar stops above it.
- Clicking a row switches tabs.
- A group made from a row starts with its name being edited, keeps the new
  name, and takes a second tab dropped on its header; both leave their
  editor-kind group. A row's drag carries the right tab.
- Keeping the sidebar open moves the editor over by the sidebar's width.
- Turning the sidebar off brings the strip back.

`sidebar_screenshots` walks the same steps and saves a PNG of each state:

```
PULSAR_SIDEBAR_SHOTS=/tmp/shots cargo test -p ui_core --lib -- nav_sidebar::tests::sidebar_screenshots
```

This needs a GPU adapter (a software one works). Text uses real fonts; icons
come out blank, because a test context has no asset source.

## Limits

- **Rail icons are the editors' tab icons.** An editor without one gets a
  generic page icon, and the level editor gets a globe.
- **Within a group, tabs keep the order they were added in.** Dragging onto a
  header moves a tab between groups but doesn't reorder it inside one.
