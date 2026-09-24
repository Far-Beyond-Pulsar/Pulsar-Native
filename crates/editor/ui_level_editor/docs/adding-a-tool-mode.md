# Adding a Level Editor Tool Mode

The level editor routes pointer events and mode-specific UI through the
`ToolMode` trait in `src/level_editor/tool_modes/mod.rs`. The built-in
`LevelEditMode` and registered `SplineMode` are current examples.

## Implement and register a mode

1. Implement `id`, labels, icon, `on_pointer`, and `clone_box`. Return
   `Consumed` from `on_pointer` when the mode handled the event; return
   `PassThrough` to allow normal selection and gizmo behavior.
2. Override `toolbar_controls`, `status`, `layout`,
   `contributes_panels`, or `build_panel` only when needed.
3. Register the mode in `register_tool_modes` in
   `tool_modes/registry.rs`. `EditorDomain::default` calls this after
   constructing the built-in registry.

The dropdown, toolbar, status bar, dock layout, and viewport dispatch use the
selected trait object. They do not need a mode-specific branch for pointer
events.

## State and UI updates

Keep state used only by the mode on the mode struct. Put state used by other
editor views in a named domain under `EditorDomain`. The cached
`ToolbarSignature` and `StatusBarSignature` must include any domain values
that affect their respective views; otherwise those views can show stale
values.

`ToolModeDispatcher::dispatch_widget_edit` currently has no generic
write-back hook for interactive toolbar widgets. A new mode that needs
clickable widgets must add its own routing or extend the trait/dispatcher.
`SplineMode` uses pointer events for edits and read-only toolbar values.

For a dock panel, return a stable `ModePanelDescriptor` from
`contributes_panels` and construct the matching `PanelView` in
`build_panel`. Keep panel construction separate from per-frame updates.

`brush_cursor` is currently not wired to a renderer overlay. A mode that
needs an on-screen cursor must connect that output to the viewport renderer.
