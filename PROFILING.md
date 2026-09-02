# Profiling (Tracy)

Function-level CPU profiling is wired through the [`profiling`](https://crates.io/crates/profiling)
crate (v1.x) so backends stay swappable. Tracy is the active backend; a second,
independent collector (the custom SQLite-backed one feeding the Flamegraph
panel) continues to run alongside it. The Inspector's Profiler tab uses
WGPUI's bounded capture engine directly.

## Build

```
cargo build --release -p pulsar_engine --features profile-tracy
```

Enabling `profile-tracy` on the engine:

- turns on `profiling / profile-with-tracy` for the **single** crates.io
  `profiling` package in the graph,
- which **feature-unifies with gpui-ce's own copy of that package**, so every
  `wgpui: ...` zone inside the render path lights up too (verified with
  `cargo tree -p pulsar_engine --features profile-tracy -e features -i profiling@1.0.18`),
- and explicitly chains `gpui-ce/profile-tracy` as belt-and-braces.

Without the feature all macros expand to nothing (`profiling` default features
provide only the proc-macros), so shipped builds carry zero instrumentation
cost. The feature is OFF by default.

## Tracy client

1. Download a client GUI from https://github.com/wolfpld/tracy/releases
   (v0.11 or newer — the workspace resolves `tracy-client` 0.18.x /
   `tracy-client-sys` 0.28.x, which speak the Tracy 0.11 protocol).
2. Launch the engine first (or the client first — either order works).
3. In the Tracy GUI press the **Connect** button (File -> Connect); it listens
   on TCP `127.0.0.1:8086` by default and will find a freshly started engine
   automatically.

If nothing connects, confirm both binaries were built from the same feature
configuration — an engine built without `--features profile-tracy` never opens
a socket.

## Expected zones

Engine-side (this repo):

| Zone | Location |
| --- | --- |
| `pulsar: init graph execute` | `crates/core/engine/src/main.rs` |
| `pulsar: gpui app run` | body of the `gpui_app.run(...)` closure, same file |

The fork-based instrumentation (`Engine::EventLoop`,
per-init-task scopes from `init/graph.rs`, named Tokio workers) feeds the
Flamegraph panel collector, not Tracy.

wgpui-side (prefix `wgpui: `, emitted by gpui-ce on the main thread):
`draw`, `apply_invalidations`, `layout`, `prepaint`, `paint`, `present`,
`present_framebuffer_only`, `record layer`, `layer composite`,
`evict_stale_layers`, `scene finish`, `resolve_orders`, `renderer draw`,
`gpu upload`. On the `retained-phase-9` wgpui branch additionally: `slab
splice`, `pack layer at record`, `flush slab run`.

## Cross-checking with WGPUI_RENDER_STATS

Run with `WGPUI_RENDER_STATS=1` and every Tracy zone above has a matching
stderr counter/timer printed once per second:

| render_stats name | Tracy zone |
| --- | --- |
| `frame: layout` | `wgpui: layout` |
| `frame: prepaint` | `wgpui: prepaint` |
| `frame: paint` | `wgpui: paint` |
| `frame: scene finish` | `wgpui: scene finish` |
| `frame: gpu upload` | `wgpui: gpu upload` |
| `layer: composite` | `wgpui: layer composite` |
| `window: draw + present` | `wgpui: draw` / `wgpui: present` |

Numbers should agree within timer overhead; if they diverge, suspect extra
frames (resize, tooltips) between dumps rather than either measurement.

On wgpui branches carrying notify attribution, `WGPUI_NOTIFY_ATTRIBUTION=1`
adds a top-10 dump of entities issuing the most `cx.notify()` calls — use it
to explain why `apply_invalidations` / `layout` spikes appear in a capture.

## Structured diagnostics

With the WGPUI flamegraph feature enabled, open the Inspector's **Profiler**
tab and stop a capture. The **Diagnostics** tab shows the selected frame's
structured lifecycle events alongside the Flame Chart, Counters, Memory, UI
Tree and GPU Deep Capture views. Resize captures include native resize
dimensions and scale, total resize-handler time, `Window::bounds_changed`,
refresh requests, drawable texture replacement, and the blocking
`Surface::configure` duration.

Diagnostic records are fixed-width values stored in a bounded queue and
attached to the next captured frame. With capture disabled, each call site
does only one relaxed atomic check; no timestamp, allocation, lock, or string
formatting occurs. The viewer limits the displayed diagnostic tail to 500
rows so a resize storm cannot make the profiler itself slow.

## Cost model

With `profile-tracy` enabled but **no client connected**, the Tracy thread is
idle-cheap (queue polling only); zones still cost a timestamp pair each. With
a client connected expect roughly low single-digit percent overhead. For
shipped builds keep the feature off — it compiles out entirely.
