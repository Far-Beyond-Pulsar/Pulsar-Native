# Theme system

Themes are JSON files in `themes/`. The engine loads them at startup via
`ui::themes::init(cx)`.

## Theme file format

Each theme file is a `ThemeSet` containing a `name`, optional `author`/`url`,
and an array of `ThemeConfig` entries:

```json
{
  "name": "My Theme Set",
  "author": "Me",
  "themes": [
    {
      "name": "My Dark Theme",
      "mode": "dark",
      "window": { "background": "opaque" },
      "colors": { ... },
      "highlight": { ... }
    }
  ]
}
```

## Required colors (~18)

```
accent.background, accent.foreground
background, border
danger.background
foreground
muted.background, muted.foreground
primary.background, primary.foreground
secondary.background, secondary.foreground
base.blue, base.cyan, base.green, base.magenta, base.red, base.yellow
```

## Optional colors (~54)

Accordion, caret, chart (1-5), danger variants, description_list, drag.border,
drop_target, group_box, info variants, input.border, link variants, list
variants, overlay, popover, progress.bar, ring, scrollbar variants,
secondary variants, selection, sidebar variants, skeleton, slider variants,
switch, tab variants, table variants, tiles, title_bar, window.border.

## Syntax highlighting

`ThemeConfig.highlight` contains:
- `editor.background`, `editor.foreground`
- `editor.active_line.background`, `editor.line_number`, `editor.active_line_number`
- Error/warning/info/success/hint colors with background and border
- `syntax` — ~40 tree-sitter token types, each mapping to a `ThemeStyle`:
  `{ color, font_style: normal|italic|underline, font_weight: 100-900 }`

Token types: attribute, boolean, comment, comment_doc, constant, constructor,
embedded, emphasis, emphasis.strong, enum, function, hint, keyword, label,
link_text, link_uri, number, operator, predictive, preproc, primary, property,
punctuation (with .bracket, .delimiter, .list_marker, .special), string (with
.escape, .regex, .special, .special.symbol), tag (with .doctype), text.literal,
title, type, variable (with .special), variant.

## Window backgrounds

```json
"window": { "background": "opaque" }
```

Options: `opaque`, `transparent`, `blurred`, `mica_backdrop`, `mica_alt_backdrop`.

## Plugin theme access

The theme pointer is passed to plugins during `_plugin_create()` as a raw
`*const c_void` and stored in a `OnceLock` inside the plugin DLL. The `ui`
crate provides `register_plugin_accessor()` to set up a thread-safe accessor.

Available themes: 24 JSON files including adventure, ayu, catppuccin, default,
everforest, fahrenheit, flexoki, gruvbox, harper, hybrid, jellybeans, kibble,
macos-classic, matrix, molokai, solarized, tokyonight, twilight, and others.
