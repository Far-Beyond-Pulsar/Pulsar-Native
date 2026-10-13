# Widget editor (`ui_editor_plugin`)

The `.widget` asset and a WYSIWYG editor for building user interfaces.

## Widgets and identity

A widget is a tree of nodes: **panels** that arrange children (canvas, vertical/horizontal box,
overlay, grid, wrap box, scroll box, size box, scale box, border), **leaves** that draw something
(text, image, button, check box, slider, progress bar, text input, combo box, spacer), a **named
slot** (a hole users of the widget can fill) and **widget** nodes, which are *other widgets*.

Widgets refer to each other by **id**, a UUID stored inside the file (`"id"`). Never by name or
path, so renaming or moving a widget file breaks nothing; the next scan finds it at its new
place. Two more ids are stable the same way: node ids (parameters point at nodes by id) and
parameter ids (instances set parameters by id, so renaming a parameter is safe).

- A file with no id (a new one from the template, or hand-made) is given one the first time it is
  opened, and the id is written at once.
- A copied file has its original's id. The library reports the id as **ambiguous** and the editor
  offers *Give this widget a new id*; nothing guesses which copy is meant.
- Deleted widgets leave a placeholder showing the name they had, never silently vanish.
- Cycles (a widget inside itself, directly or through others) are refused when adding and flagged
  in Issues.

Images are referenced by project-relative path: the engine has no id service for file assets yet.

## Parameters and slots

A widget can declare **parameters** (typed values) that drive node properties. A widget node
sets them per instance, or passes one of its own parameters through. A **named slot** is filled
with the children of the widget node that uses it.

## The editor

| Panel | What |
|-------|------|
| Designer | the widget drawn on a screen of any preview size/scale; select, move, resize, marquee, snap, align, drag-and-drop, zoom/pan, safe zone, button-state preview |
| Palette / Widgets | kinds of widget to add; the project's widgets to nest |
| Hierarchy | outline with rename, reorder/reparent by drag, hide/lock, search, context menu |
| Details | schema-driven properties, layout (anchor picker), instance parameters, widget settings |
| Parameters | declare parameters and see what they drive |
| Issues | validation with one-click repairs |

Every edit is one undo step (a whole drag is one). The designer paints through GPUI's renderer
from a plain display list, so the layers below `editor` are GPUI-free and usable by a runtime:
`model` -> `library` -> `resolve` -> `layout` -> `display`.
