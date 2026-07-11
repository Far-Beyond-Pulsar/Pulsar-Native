# Profiling - Ultra-Fast Instrumentation-Based Tracing

## Why This Instead of Sampling?

### Sampling (Old Way - dtrace_profiler)
- ❌ Interrupts threads randomly
- ❌ Inaccurate timing
- ❌ Misses short functions
- ❌ High overhead on Windows
- ❌ Requires admin/special permissions

### Instrumentation
- ✅ **EXACT timing** for every instrumented function
- ✅ **Zero interruption** - no thread suspension
- ✅ **Minimal overhead** - just timestamp capture
- ✅ **Named threads** - human-readable traces
- ✅ **Perfect nesting** - see exact call stacks
- ✅ **Works everywhere** - no permissions needed

## Quick Start

### 1. Add to your system's Cargo.toml

```toml
[dependencies]
profiling = { path = "../../crates/profiling" }
```

### 2. Instrument your code

```rust
use profiling::{profile_scope, set_thread_name};

fn main() {
    // Enable profiling
    profiling::enable_profiling();
    
    // Name your threads!
    set_thread_name("Main Thread");
    
    game_loop();
}

fn game_loop() {
    profile_scope!("GameLoop");
    
    update_physics();
    render_frame();
}

fn update_physics() {
    profile_scope!("Physics::Update");
    
    {
        profile_scope!("Physics::BroadPhase");
        // ... collision detection
    }
    
    {
        profile_scope!("Physics::Integrate");
        // ... integrate velocities
    }
}

fn render_frame() {
    profile_scope!("Render::Frame");
    // ... rendering code
}
```

### 3. Get the data

```rust
// Collect events (clears internal buffer)
let events = profiling::collect_events();

// Or get all events without clearing
let all_events = profiling::get_all_events();

// Send to flamegraph viewer
ui_flamegraph::convert_profile_events_to_trace(&events, &trace_data)?;
```

## Macro Reference

### `profile_scope!(name)`
Times everything until the end of the current scope:
```rust
{
    profile_scope!("MyFunction");
    // Timed code here
} // Automatically records duration
```

### `set_thread_name(name)`
Give threads human-friendly names:
```rust
std::thread::spawn(|| {
    profiling::set_thread_name("Worker 1");
    // ... work
});
```

## Best Practices

### ✅ DO:
- Instrument coarse-grained operations (functions taking >0.1ms)
- Name threads clearly: "Render Thread", "Physics Worker 1", etc.
- Use static string literals when possible: `profile_scope!("Update")`
- Instrument at system boundaries (physics, render, audio, etc.)

### ❌ DON'T:
- Instrument tiny functions called millions of times
- Instrument inside tight inner loops
- Use dynamic strings unnecessarily (they allocate)
- Leave profiling enabled in shipping builds

## Performance

- **Overhead**: ~10-50ns per scope (just two timestamp reads)
- **Memory**: ~80 bytes per event
- **Thread-safe**: Lock-free event submission
- **Scalable**: Handles millions of events efficiently

## Integration with Flamegraph UI

The profiling crate is designed to work seamlessly with `ui_flamegraph`:

```rust
use profiling;
use ui_flamegraph::TraceData;

// Collect profiling data
let events = profiling::collect_events();

// Convert to trace format
let trace_data = TraceData::new();
ui_flamegraph::convert_profile_events_to_trace(&events, &trace_data)?;

// Now view in flamegraph window!
```

## Comparison to Other Solutions

| Feature | This Crate | dtrace_profiler | puffin | tracy |
|---------|-----------|-----------------|--------|-------|
| Instrumentation | ✅ | ❌ | ✅ | ✅ |
| Zero overhead when disabled | ✅ | ✅ | ❌ | ❌ |
| Named threads | ✅ | ✅ | ✅ | ✅ |
| No external tools | ✅ | ❌ | ✅ | ❌ |
| Exact timing | ✅ | ❌ | ✅ | ✅ |
| Built for Pulsar | ✅ | ✅ | ❌ | ❌ |

## Future Enhancements

- [ ] GPU timing events
- [ ] Memory allocation tracking
- [ ] Custom counters/graphs
- [ ] File-based trace export
- [ ] Live streaming to viewer
- [ ] Conditional compilation (#[cfg(feature = "profiling")])

## License

Same as Pulsar-Native
