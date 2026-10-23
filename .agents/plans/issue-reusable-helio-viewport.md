## Extract a reusable Helio viewport from `ui_level_editor`

### What we did

The Render Graph Editor (`plugins/vendor/render_bps`) has a live preview above its Properties panel: a full Helio renderer on the editor's shared wgpu device, drawing a private built-in test scene through the graph being edited (`render_bps/src/preview.rs`, `PipelinePreview`).

The level editor's `HelioViewport` (`crates/editor/ui_level_editor/src/ui/viewport/helio_viewport/`) is the obvious thing to reuse, but it is tied to `LevelEditorState` (loading overlay, thumbnails, pending errors, the level editor's input latch and frame pump). So for now `PipelinePreview` is a small separate component that duplicates the core of it:

- lazy `WgpuSurfaceHandle` creation on the window, render thread, `prepare_for_render` before the submit guard, `present_synced_silent`, a frame pump that repaints on published frames;
- its own orbit/zoom camera through `GpuRenderer::set_editor_camera_state`;
- simple ~60 Hz sleep pacing instead of `FramePacer`.

### What is missing there compared to `HelioViewport`

- display-refresh-aware frame pacing and the consumer-wait backpressure;
- init queue (cold pipeline compiles are not serialized with other viewports);
- device-loss recovery beyond adopting the surface's flag;
- input (camera keys, picking, gizmos), debug views, thumbnails, loading overlay.

### To do

Pull the surface + render thread + frame pump out of `HelioViewport` into a crate both the level editor and `render_bps` use, parameterised over what is level-editor-specific (loading state, error sink, input), then replace `PipelinePreview`'s copy with it.

Related: `.agents/RENDER_GRAPH.md` ("Preview").
