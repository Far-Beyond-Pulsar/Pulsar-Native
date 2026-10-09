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

- **Editors.** Pinned tabs come first, then the rest grouped by the kind of
  editor (every blueprint together, every material together), in the order
  they were opened. Hover a row to pin it or close it. A pinned file stays
  listed after its tab is closed, dimmed, and reopens in one click. Groups
  collapse.
- **Content.** The project's `Content` folder, or the project folder when it
  has none. `target/` is not listed. The tree updates as files change on
  disk, because it is the file drawer's own tree.

Pins, collapsed groups and expanded folders are saved per project in
`.pulsar/layout.json`, next to the dock layout.

## How it is built

- `crates/editor/ui_core/src/app/nav_sidebar/`
  - `model.rs`: the ordering and pinning rules, the folder rows, the saved
    form, and the hover state machine. It is plain data with unit tests.
  - `mod.rs`: the connection to the dock. It lists the centre tabs across
    splits, activates and closes tabs, applies the setting, and lists a
    folder in the drawer.
  - `render.rs`: the rail, the sidebar, the rows and the tree.
  - `tests.rs`: an editor window test (see below).
- The tab strip is hidden with `TabPanel::set_tab_bar_hidden`
  (Far-Beyond-Pulsar/WGPUI-Component#24).
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
- A folder is listed in the drawer.
- Clicking a row switches tabs.
- Keeping the sidebar open moves the editor over by the sidebar's width.
- Turning the sidebar off brings the strip back.

`sidebar_screenshots` walks the same steps and saves a PNG of each state:

```
PULSAR_SIDEBAR_SHOTS=/tmp/shots cargo test -p ui_core --lib -- nav_sidebar::tests::sidebar_screenshots
```

This needs a GPU adapter (a software one works). It uses real fonts and the
editor's icons, through `TestAppContext::set_asset_source`
(Far-Beyond-Pulsar/WGPUI#266).

## Not done yet

- **Tab groups are automatic**, by editor kind. The issue also mentions groups
  the user makes; that would need naming and drag and drop between groups.
- **No tab dragging.** With the strip hidden, tabs can't be dragged into
  splits. Splits made before switching still work and are listed.
- **The rail's icons are the editors' tab icons.** Editors without one get a
  generic page icon, and the level editor gets a globe.
- **The hover sidebar covers the left of the file drawer** while it is open.
  It closes as soon as the pointer moves on to the assets.
