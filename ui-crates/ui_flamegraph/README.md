# Flamegraph Tracing UI

High-performance flamegraph visualization for profiling and tracing data in Pulsar Native.

## Features

- **Hyper-Efficient Rendering**: Uses GPUI's batched quad rendering system to display thousands of trace spans with smooth 60+ FPS performance
- **Interactive Zooming & Panning**: 
  - Ctrl/Cmd + Scroll to zoom in/out
  - Scroll to pan vertically
  - Shift + Scroll to pan horizontally
- **Color-Coded Spans**: 16-color palette automatically assigned to distinguish different trace categories
- **Viewport Culling**: Only renders visible spans for maximum performance
- **Real-time Updates**: Thread-safe trace data structure for live profiling

## Architecture

### Performance Optimizations

1. **Batched Rendering**: All spans are rendered in a single paint layer using GPUI's scene batching system
2. **Viewport Culling**: Calculates visible time and depth ranges to skip off-screen spans
3. **Minimal Allocations**: Uses efficient data structures and cloning strategies
4. **Direct GPU Rendering**: Leverages GPUI's `paint_quad` for hardware-accelerated rectangle rendering

### Key Components

- `FlamegraphView`: Main rendering component with pan/zoom state
- `TraceData`: Thread-safe container for trace spans using `Arc<RwLock<TraceFrame>>`
- `TraceSpan`: Individual profiling span with name, timing, depth, and color
- `TraceFrame`: Collection of spans with cached min/max time and depth

## Usage

```rust
use ui_flamegraph::{FlamegraphWindow, TraceData, TraceSpan};

// Create trace data
let trace = TraceData::new();

// Add spans
trace.add_span(TraceSpan {
    name: "Frame Update".into(),
    start_ns: 0,
    duration_ns: 16_600_000, // 16.6ms
    depth: 0,
    thread_id: 1,
    color_index: 0,
});

// Open flamegraph window
FlamegraphWindow::open(trace, cx);
```

## Controls

- **Zoom**: Ctrl/Cmd + Mouse Wheel
- **Pan Vertical**: Mouse Wheel
- **Pan Horizontal**: Shift + Mouse Wheel

## Technical Details

### Rendering Strategy

The flamegraph uses GPUI's canvas element with two phases:

1. **Prepaint**: Captures viewport dimensions and clones trace data
2. **Paint**: Iterates visible spans and calls `window.paint_quad()` for each

This approach allows GPUI to batch all quads into a single draw call for optimal GPU performance.

### Data Structure

```rust
pub struct TraceSpan {
    pub name: String,
    pub start_ns: u64,
    pub duration_ns: u64,
    pub depth: u32,
    pub thread_id: u64,
    pub color_index: u8,
}
```

Spans are stored in a flat `Vec` and culled based on:
- Depth range (visible rows)
- Time range (horizontal viewport)

### Future Enhancements

- Span selection and detailed info panel
- Search/filter by span name
- Export to Chrome tracing format
- Multi-threaded timeline view
- Flame chart mode (time-ordered)
