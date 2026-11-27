# Settings

How settings are registered, stored, shown and read. Code: `crates/third-party/pulsar-config`
(the database), `crates/core/pulsar_settings` (registrations), `crates/core/engine_state`
(`settings`, `plugin_settings`: files), `crates/editor/ui_settings` (the windows),
`crates/editor/ui_common/src/appearance.rs` (appearance applied to the theme).

## Namespaces and who may register

Every setting lives in one `pulsar_config::ConfigManager` (`engine_state::settings::global_config()`),
under a namespace, an owner path and a key.

| Namespace | What | Registered by | Saved in | Edited in |
|---|---|---|---|---|
| `editor` | the editor's own settings, for every project | core only: `pulsar_settings::editor` | the editor config dir (`GlobalSettings`) | **Editor Settings** |
| `project` | the open project's settings | core (`pulsar_settings::project`: `graphics`, `streaming`, Helio's `rendering`) and plugins (`ProjectSettingsRegistration`, `SettingsGroupRegistration`) | `<project>/.pulsar` (`ProjectSettings`) | **Project Settings** |
| `plugin` | a plugin's own groups | plugins: `pulsar_settings::plugin::PluginSettings`, and groups the host derives (`SettingsGroups`: every language server's) | `<project>/.pulsar/plugin/<owner>.toml`, only the keys the project overrides (`engine_state::plugin_settings`) | **Project Settings** |

Plugins are project relative: there is no registration a plugin can use to add an
`editor` setting. A renderer setting a plugin adds is a `helio_settings::Group` registered with
`SettingsGroupRegistration` (Helio owns every host setting's description, see the
`helio-architecture` skill); a plugin's other project settings use `ProjectSettingsRegistration`
(the materials' `texture_sampling`, the meshes' `virtual_geometry`) or `PluginSettings`.

`pulsar_settings::project::owner_origin(owner)` names the plugin whose registration added a
project owner (`None` for the engine's own); the Project Settings window files the owner under
that plugin.

A setting is registered only when something reads it. Before adding one, add its reader; when
the reader goes, the setting goes.

## The windows

`ui_settings` has two windows over one `SettingsScreen`:

- **Project Settings** (`ProjectSettingsWindow`, `"ProjectSettingsWindow"`; the Project menu,
  the app menu's Project Settings, the language server footer's "Settings..." which opens a
  plugin's page): the engine's project pages at the top level (Graphics, Rendering,
  Streaming), then a **Plugins** folder with one page per enabled plugin that has settings
  (its project owners and its `plugin` groups together).
- **Editor Settings** (`EditorSettingsWindow`, `"EditorSettingsWindow"`; the app menu's Editor
  Settings, the command palette): one page per `editor` owner.

The navigation tree is at most three levels deep: the Plugins folder, a plugin's page, the
page's sections. A section is a schema `page` (the text after the last ` / `); a page with one
section lists none under it. The tree is a `uniform_list` over the flattened visible rows
(`utils::model::tree_rows`). The page is a `gpui::list` with `measure_all`: each row (page
header, section heading, setting) is measured, so a long description makes its row taller
instead of overlapping the next, and a section jump scrolls to the heading's measured row (on
another page, after that page's first layout). Scrolling highlights the section in view.

Search filters both: a page or section whose title matches keeps all of it, otherwise only the
matching settings (label, key, description) stay, and every folder opens.

Controls by `FieldType`: switch, dropdown (over string or integer values), number input
(clamped, stepped; integers stay integers), text input (a list is comma separated), color
picker. A text, number or color control's editor state is held per setting (namespace, owner,
key), never per row index. A changed setting shows a dot and a reset button. Settings tagged
`TAG_REQUIRES_RESTART` say so. Save writes the window's files (the editor's, or the project's
and its plugin overrides).

Appearance (`editor.appearance`: theme, color-blind mode, font size, corner radius,
scrollbars) applies at once: `ui_common::appearance::apply` runs at startup and after any change
in the window. The title bars' light/dark toggles persist the theme through
`ui_common::appearance::set_theme_mode`.

## The Game page (`Pulsar/project.json`)

The Project Settings window's first page, **Game**, edits two keys of the project file the game reads,
not the settings database: **Default Level** (`startup_level`; absent shows the level that would load,
marked "(default)"; None writes `""`, "no level"; reset removes the key) and **Game Instance**
(`game_instance`; a class folder; None or reset removes it). Both are asset pickers
(`ui_common::AssetPicker` in a popover, with a None button); the game instance picker offers only the
class folders that implement the Game Instance trait (`utils::game::game_instance_classes`, from the
project's trait index). Edits are staged like other settings (changed dot, reset, Save bar); Save
re-reads the file and writes only the keys edited, keeping every other key (`utils::game::GameSettings`,
`pulsar_content::edit_project_file`). What the keys mean: `GAMEPLAY.md`, "The game instance and level
switching".

## Reading a setting

- Core: `global_config().get(ns, owner, key)`, or the owner module's consts.
- A renderer group: `project_settings::resolved(&GROUP)` and the feature's `from_settings`.
- A `plugin` group: `pulsar_settings::plugin::handle(origin, id)` (`get_*`, `on_change`).
- Editors a plugin provides get the `editor.code_editor` values in the
  `EditorSettingsSnapshot` of their `EditorContext`.

## Viewport frame policy (`editor.viewport`, live)

Two settings of the Viewport page decide how often a visible viewport draws (the policy itself is
`engine_backend::services::render_runtimes::PacingPolicy`, see `.agents/RENDER_RUNTIMES.md`,
"Frame policy"); both apply within a quarter of a second, with no restart:

| Key | Default | What |
|---|---|---|
| `unfocused_fps` | 8 | Frames a second a visible viewport draws while its window is not focused. 0 pauses it until the window is focused again. A minimized window and a hidden tab always draw nothing |
| `ambient_fps` | 15 | Frames a second a focused viewport draws when only content that moves on its own changes (wind, water, particles). 0 keeps the display's rate: the viewport free-runs. A camera move, an edit or streaming content always draws at the display's rate |
