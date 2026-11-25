# Render runtimes

The engine owns every Helio renderer it runs. Helio is a library: it builds a
renderer, runs a pipeline and draws a frame into a view, and knows no thread,
task or plugin. `engine_backend::services::render_runtimes` (GPUI-free, the
`render` feature, so a game build has it too) owns the renderers and draws them.

## Asking for a runtime

```rust
use engine_backend::services::render_runtimes::{self, RuntimeDescription, UpdateMode};

let runtime = render_runtimes::global().create(
    RuntimeDescription {
        label: "Render graph preview".into(),
        origin: pulsar_plugins::origin!(),   // the plugin (or engine crate) that owns it
        scene_label: "preview test scene".into(),
        priority: 0,                        // higher draws first in a pass
        mode: UpdateMode::WhenVisible,
    },
    renderer,                               // Arc<Mutex<GpuRenderer>>: scene, camera, mailbox
);
runtime.set_output(Arc::new(ui_helio_viewport::SurfaceTarget::new(surface, frames, refresh)));
```

The handle (`RuntimeHandle`, cheap to clone) is the owner's control:

| Call | Effect |
|---|---|
| `set_output(target)` | Draw into `target` from now on (a new surface, a lost device's replacement) |
| `pause()` | Stop drawing; the renderer and its GPU resources stay |
| `stop()` | Stop drawing and free the renderer's GPU resources; the description stays |
| `resume()` | Draw again (after a stop the renderer builds again, as a task) |
| `set_mode(mode)` | `Always`, `WhenVisible`, `OnDemand(n)`, `Disabled` (Godot's viewport update modes) |
| `request_frames(n)` | `n` more frames for an on-demand runtime |
| `set_visible(bool)` | Whether its output is shown (a tab in front); `WhenVisible` draws only then |
| `set_before_frame(f)` | Called right before each frame on the scheduler's thread (late input sampling) |
| `reset_history()` | The next frame starts its temporal history over |
| `renderer()` | The `GpuRenderer`, for what the owner drives (camera, selection, mailbox) |

Dropping the last handle destroys the runtime (its renderer is freed once a
frame in flight finished). A runtime whose origin's plugin is no longer enabled
is destroyed when the plugin set changes. `RenderRuntimes::shutdown` stops the
scheduler and joins it.

## States

`Starting` (no output yet, or its renderer is being built), `Running`, `Paused`
(GPU resources kept), `Stopped` (GPU resources freed, description kept), `Lost`
(its output's device was lost; it builds again on the next output from
`set_output`) and `Failed(reason)`.

## Outputs (`FrameTarget`)

| Output | Where | What |
|---|---|---|
| `ui_helio_viewport::SurfaceTarget` | editor | wgpui's triple buffer (`WgpuSurfaceHandle`): draws under its submit permission, publishes silently, bumps the view's `frames_published` so its frame pump repaints only that view. Not ready while the compositor has not taken the last frame (bounded by `FRAME_CONSUMER_WAIT_TIMEOUT`, the fast-blit path never advances it); paced at the display refresh |
| `render_runtimes::OffscreenTarget` | engine | a texture on the engine's device with an async readback (`take_image`), for thumbnails and captures; never blocks |

A new kind of output implements `FrameTarget` (device, queue, format, size,
`is_lost`, `ready`, `frame_interval`, `draw`, and `set_ready_waker` when it
can say the moment it becomes ready again: `SurfaceTarget` wakes the
scheduler when the compositor takes its frame, through wgpui's
`set_consumed_waker`, so the next frame starts then rather than a poll later).

## The scheduler

One persistent task, **Helio runtimes** (role Render), draws every runtime:
each pass it takes the runtimes in priority order and draws each that is
running, wants frames (its mode and visibility), is due at its output's pace and
whose output is ready; then it sleeps until the next one is due or something
wakes it (a new output, a mode change, a frame request, an output's consumer
taking a frame). It never waits for a consumer: an output that still holds its
last frame is skipped this pass. While it runs it holds the system timer at
1 ms (`timeBeginPeriod` on Windows): its sleeps otherwise wake on the 15.6 ms
tick, which capped every viewport at 64 Hz. Each frame it hands the renderer
its output's interval as the frame budget (`GpuRenderer::set_frame_budget`,
Helio's `PassContext::frame_budget_ms`), so passes that stream work in keep
the display's pace. A
renderer's first build (seconds: pipelines compile) runs as a pooled task,
**one build at a time** (a split layout's viewports would otherwise contend in
the driver), so other runtimes keep drawing. A pipeline swap or a
graph-affecting settings change rebuilds at the runtime's next frame boundary on
the scheduler.

Unreal draws every viewport from its one render thread; Godot's
`RenderingServer` draws all viewports in order each frame. Per-viewport loops on
one device contend for the same queue and submit lock with no parallelism to
gain, so Pulsar has none (`architecture/raw-threads-allowed.txt` lists no render
loop).

## The engine's device

`RenderRuntimes::offer_device` (the first `SurfaceTarget` offers the window's
device) and `RenderRuntimes::device` (the offered one, else one made for the
process, headless) are the one device for GPU work with no output of its own:
the render-graph pin probe and thumbnails. Nothing else creates a device.

## Visible in the Tasks window

Each runtime is a threadless persistent task (`editor_task_queue::PersistentTask`,
category `render.runtime`): its log is what its renderer logs while it draws
(the scheduler enters its span), its phase is its state, its heartbeat is one
per frame drawn (idle while it draws nothing on purpose) and its details
(`TaskContext::set_details`: owner, scene, size, mode, state, visible, frame
time, frames) are what the Tasks window's **Renderers** view lists.

## Host-driven runtimes (`DrivenRuntime`)

A game draws its frame as the last step of its tick (simulate, then draw what was
simulated), on the window's event loop (standalone) or the editor's PIE tick. Its
renderer is a `DrivenRuntime`: the service lists it (a persistent task in the
Renderers view, a heartbeat and the CPU time per frame, its size), and the
controls reach it: `begin_frame()` returns `None` while it is paused, and the
host draws nothing. Moving a game's draw onto the scheduler would split its tick
across threads (UE's game/render thread split, Bevy's pipelined rendering); that
is a game-loop design of its own, not this service's.

## Who uses it

| Owner | Runtime |
|---|---|
| `ui_level_editor` `HelioViewport` | "Level viewport" (priority 10) and each "Split viewport" (5), `WhenVisible`; visibility from the level editor's tab and the view's dock tab |
| `render_bps` `PipelinePreview` | "Render graph preview", `WhenVisible` on its panel |
| `pulsar_game` `EmbeddedGame` (PIE) | "Play in Editor", driven by the PIE tick |
| `pulsar_game` windowed app | "Game window N", driven by the window's event loop |

Not runtimes, by design: `helio-snapshot`'s `render_snapshot`/`SnapshotBatch` are
standalone tools (examples) that own their device as any host does; the editor's
asset thumbnails use its CPU `render_preview`. The materials graph editor's
preview, the SKM editor's viewport and Matter's canvas draw with their own wgpu
code on the window's device (see the p23 section of
`.agents/PLUGIN_API_REFACTOR_PLAN.md` for what moves next).

## Editor selection guides

The lights plugin registers a persistent `EditorLinesRegistration` layer in
editor builds. It reads SceneDB's full selection, resolves the selected
lights' world poses, and supplies the existing Helio debug pass with range
spheres, spot cones, direction markers, and authored source dimensions.
Property, transform, and selection edits refresh the layer when its geometry
changes. Clearing selection removes the layer.

## Frame policy: who draws how often

One module decides it for every runtime (`render_runtimes/pacing.rs`, `PacingPolicy::pace`), so no
viewport carries a timer or a rate of its own. An `UpdateMode::WhenVisible` runtime's pace is a function
of three facts: whether its output is shown (`RuntimeHandle::set_visible`: the tab in front), where its
window stands (`RuntimeHandle::set_presence(Presence { focused, minimized })`) and what its last frame
was for (`GpuRenderer::frame_kind`, `FrameKind`).

| State | Draws |
|---|---|
| hidden tab, panel collapsed | nothing: the runtime is quiet, its task idle, no timer |
| window minimized | nothing (the same) |
| window not focused | at `editor.viewport.unfocused_fps` (default 8; 0 pauses) |
| focused, `Interactive` (the camera moved, the scene's revision changed, an editor command is pending, content is streaming in, the temporal resolve is settling, a resize) | at the output's pace |
| focused, `Ambient` (only content that moves on its own: wind, water, particles) | at `editor.viewport.ambient_fps` (default 15; 0: the output's pace, free-run) |
| focused, `Idle` (the renderer found nothing to draw and skipped the frame) | a 60 Hz poll for the next change; no GPU work, no published frame |

The renderer already knew whether it had anything new to draw (`detect_activity`: the SceneDB
`World::revision`, the camera's velocity, the pending editor commands, the temporal resolve's budget,
the registered animated content); it now says which kind of frame it drew, and the scheduler reads it
after each draw. Owners tell the service about the window with `ui_helio_viewport::follow_window` (the
level viewport, the render graph preview): it sets the presence now and on every activation change,
and a change wakes the scheduler, so a window that is focused again draws on the next pass with its
renderer and pipelines exactly as they were (nothing rebuilds, no first-frame hitch). Modes other than
`WhenVisible` (`Always`, `OnDemand`) and the driven game runtimes are the owner's pace, not the policy's.
`PacingPolicy` is a pure function with tests (`pacing::tests`); the settings are in `.agents/SETTINGS.md`.
