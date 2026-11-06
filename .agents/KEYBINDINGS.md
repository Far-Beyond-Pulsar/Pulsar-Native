# Keybindings

The Keyboard Shortcuts window (`Help > Shortcuts`, `Ctrl+K Ctrl+S`) and the
per-project overrides it writes. Code: `crates/editor/ui_keybindings`.

## File

`<project>/.pulsar/keybindings.json`, written atomically (sibling file, then
rename):

```json
{
  "version": 1,
  "bindings": [
    { "keystrokes": "ctrl-j", "action": "app::ToggleFileManager" },
    { "keystrokes": "ctrl-space", "action": null },
    { "keystrokes": "ctrl-e", "action": "app::Run", "context": "Editor" }
  ]
}
```

Entries apply in order over the defaults; later ones win. `action: null`
unbinds the keystrokes **in that context** (no `context` = the bindings that
have none; other contexts are untouched). `params` carries the parameters of an
action that takes some.

Reading is forgiving: a missing file is empty; invalid JSON or a bad entry is
skipped with a warning and the original kept as `keybindings.json.bak`; a file
with a newer `version` is read-only and never overwritten. Keystrokes and
contexts are normalised on load (`ctrl-k` and `Ctrl-k` are one spelling).

## Startup

`PulsarApp::restore_layout_now` calls `apply_project_keybindings` once the
plugins are loaded, so every default binding (the editor's and the plugins')
already exists. Applying is idempotent and a rebuild, never an append, so
windows and project switches can call it again.

## How overrides reach the keymap (`utils/registry.rs`)

`apply` rebuilds the keymap from its *base* bindings plus the overrides, in
order. User bindings carry `KeyBindingMetaIndex(0)` (`USER_META`), which is how a
rebuild tells them from everything else, so no copy of the defaults is stored and
bindings a plugin adds later survive. `0` is also the value the keymap itself
reads as "a user binding".

**Unbinding removes the binding; it does not add a `NoAction`.** At dispatch a
context-less binding outranks contextual ones, so a `NoAction` for a global
shortcut would silence contextual bindings sharing its keys (a text field's own
Ctrl+A). Removed defaults go to a `RemovedDefaults` global so a later rebuild
(reset, another edit) can bring them back.

## Layers

| File | What |
|---|---|
| `utils/overrides.rs` | Pure: `Override`, `Entry`, `effective`, `add_binding` / `remove_binding` / `reset_action`, clash and shadow detection, `compact` (drops overrides that change nothing, so remove + re-add leaves the file as it was) |
| `utils/file.rs` | Load / save the file |
| `utils/registry.rs` | GPUI bridge: read the keymap and action registry, `apply`, name humanising |
| `utils/store.rs` | `KeybindingStore` global (current project, overrides, warnings), `apply_project_keybindings` |
| `utils/view_model.rs` | One entry per action with its live bindings; search and filters |
| `screen.rs`, `components/`, `handlers/` | The window: toolbar, virtualized list, recorder dialog, status bar |

## Using the window

Every registered action is listed (found through the action registry: nothing
to register by hand), grouped by namespace, with its doc comment. Click a
shortcut to change it, `+` to add another, the cross to remove one, the undo
arrow to restore an action's defaults. The recorder captures real keystrokes
(`Esc` cancels, `Enter` saves, `Backspace` removes the last key; several keys
in a row make a chord) and an optional context. It lists what the keys clash
with; saving takes them from the other action. Search matches titles,
namespaces, descriptions and keys (`ctrl+k`). Filters: All, Changed, Unbound,
Conflicts. Every change is saved and bound immediately.

Actions that need parameters (such as `CreateAsset`) are listed but cannot be
bound from the window; put them in the file with `params`.

## Not done

- The file is not watched: edits made outside the window apply at the next
  start (or when a window changes something).
