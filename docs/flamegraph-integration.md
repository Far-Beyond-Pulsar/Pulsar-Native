# Flamegraph Integration

The flamegraph profiler has been integrated into the main Pulsar Native engine UI with a comprehensive trace visualization system.

## Accessing the Flamegraph

### Via Status Bar Button

Click the **Activity icon** (📊) in the bottom status bar to open the flamegraph profiler window.

### Via Keyboard Shortcut

*(Keyboard shortcut to be configured)*

### Via Action

Dispatch the `ToggleFlamegraph` action from code:

```rust
cx.dispatch_action(Box::new(ToggleFlamegraph));
```

## Current Implementation

The flamegraph generates **realistic large-scale trace data** simulating a full game engine:

### Generated Data
- **200 frames** of execution (3.32 seconds total)
- **Thousands of spans** across multiple depth levels (up to 5 levels deep)
- **6 major engine systems**: Update, Render, Physics, Audio, Network, AI
- **Variable timing**: Randomized durations within realistic ranges
- **Deep nesting**: Complex call stacks with subsystems, processes, and atomic operations

### Systems Simulated
- **Update**: Entity updates with 2-15 subtasks (2-8ms)
- **Render**: Draw calls with 3-20 subtasks (3-10ms)
- **Physics**: Collision detection with 2-12 subtasks (0.5-4ms)
- **Audio**: Channel mixing with 2-8 subtasks (0.1-1ms)
- **Network**: Packet processing with 2-6 subtasks (0.05-0.5ms)
- **AI**: Pathfinding with 2-10 subtasks (0.2-3ms)

Each system generates nested operations up to 5 levels deep, creating a realistic flamegraph with:
- Level 1: Major systems
- Level 2: Subsystems (e.g., "Entity Update 5", "Draw Call 12")
- Level 3: Processes (e.g., "Process 3")
- Level 4: Atomic operations (e.g., "Atomic 1")
- Level 5: Micro operations (e.g., "Micro 0")

## UI Features

### Window
- **Title Bar**: "Flamegraph Profiler"
- **Resizable**: Minimum 600x400, default 1200x800
- **Theme Integration**: Uses application theme colors

### Visualization
- **Viewport Culling**: Only renders visible spans for smooth performance
- **Color Coding**: 16-color palette for visual differentiation
- **Status Bar**: Shows total span count, duration, and current zoom level

## Controls

- **Ctrl/Cmd + Scroll**: Zoom in/out (0.1x to 100x)
- **Mouse Wheel**: Pan vertically through depth levels
- **Shift + Scroll**: Pan horizontally through time

## Performance

With thousands of spans rendered:
- Viewport culling ensures only visible spans are drawn
- Batched rendering via GPUI's scene system
- Smooth 60+ FPS with full interactivity

## Future Integration

The flamegraph will be integrated with:
- Real-time engine profiling data via tracing instrumentation
- Rust-analyzer operation tracking
- Custom span recording API for user code
- Export to Chrome tracing format (.json)
- Thread-based filtering and multi-timeline view

See `ui-crates/ui_flamegraph/README.md` for technical implementation details.

