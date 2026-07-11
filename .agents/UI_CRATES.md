# UI Crate conventions

The flat-layout pattern established during the `ui_file_manager` refactor.
Applies to all editor crates in `crates/editor/`.

## One level deep, no sub-crates

Every UI crate is a single `Cargo.toml` with source in `src/`.  No nested
sub-crates, no `lib.rs` that re-exports a deep tree.  The module hierarchy
is at most one directory deep:

```
src/
  lib.rs              — crate root, re-exports
  window.rs           — optional window wrapper
  [other flat .rs]    — simple crates can stay flat

  components/         — Render logic (what you see)
    mod.rs            — struct definition, constructor, Render impl
    …                 — one file per visual section

  handlers/           — Event handlers (what happens)
    mod.rs            — free functions wired to GPUI actions
    …                 — larger handlers split into files

  utils/              — State, helpers, glue
    mod.rs            — re-exports
    actions.rs        — GPUI #[derive(Action)] structs
    types.rs          — shared types, enums, event structs
    …                 — one concern per file
```

Not every crate needs all three directories.  A simple panel might just have
`lib.rs` plus a few flat modules.  When you _do_ reach for subdirectories,
use `components/`, `handlers/`, `utils/` — nothing else.

## Module roles

| Directory | Contents | Example |
|---|---|---|
| `components/` | The entity struct, `Render`, `EventEmitter`, `Focusable` | `FileManagerDrawer`, grid/list views |
| `handlers/` | Free functions called by `.on_action()` or `.on_event()` | `handle_item_click`, `handle_delete_item` |
| `utils/` | Types, actions, helpers, file I/O, state that doesn't touch GPUI rendering | `FileItem`, `CreateAsset`, `FsMetadataManager` |

## Struct layout

- All fields are `pub(crate)` — handler and util modules access them directly
  (same-crate modules, no getter/setter boilerplate).
- Only expose `pub` fields when downstream crates need them (e.g.
  `project_path`).
- Constructor takes `window, cx` params so it can create child entities and
  subscribe to events inline.

## Actions

GPUI action structs live in `utils/actions.rs` (or a standalone `actions.rs`
for simpler crates).  One `#[derive(Action)]` per struct, all in the same
`#[action(namespace = …)]`.

```rust
#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct CreateAsset {
    pub file_type_id: String,
    pub display_name: String,
    pub extension: String,
    pub default_content: Value,
}
```

## Event handlers

Handler functions take `d: &mut Drawer, …` (the entity as first param) and are
called via `crate::handlers::handle_foo`.  They are **not** methods on the
struct — keeping them as free functions in `handlers/` makes the `Render` impl
in `components/` a thin wiring layer.

Exception: `set_project_path` is a method because upstream crates call it via
`Entity::update` and method syntax is cleaner at the call site.

## Exports

`lib.rs` explicitly re-exports the public API:

```rust
pub mod components;
pub use components::FileManagerDrawer;
pub use utils::{FileSelected, FolderNode};
```

Internal modules (`handlers`, utilities used only within the crate) are
declared `mod handlers;` (no `pub`).
