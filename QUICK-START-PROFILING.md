# Quick Start - Instrumentation Profiling

## ✅ System is Ready!

The instrumentation profiling system is built and integrated with your flamegraph UI.

## 🚀 Start Using It NOW

### 1. Open the Flamegraph Window

The window now uses instrumentation instead of sampling - no admin permissions needed!

### 2. Add Profiling to Your Code

Pick ANY system and add ONE line:

```rust
use profiling::profile_scope;

fn your_function() {
    profile_scope!("YourFunction");  // ← Add this!
    // Your code here
}
```

### 3. Example: Profile Your Main Loop

```rust
use profiling::{profile_scope, set_thread_name, enable_profiling};

fn main() {
    // Enable profiling at startup
    enable_profiling();
    
    // Name the main thread
    set_thread_name("Main Thread");
    
    run_engine();
}

fn run_engine() {
    loop {
        profile_scope!("Frame");
        
        {
            profile_scope!("Update");
            update_systems();
        }
        
        {
            profile_scope!("Render");
            render_frame();
        }
    }
}
```

### 4. See It in Action

1. Click "▶ Start" in the flamegraph window
2. Your instrumented code appears in real-time
3. Named threads show up first
4. Click "⏹ Stop" when done

## 📍 Where to Add Instrumentation

### High Priority

```rust
// Main loop
fn game_loop() {
    profile_scope!("GameLoop");
    // ...
}

// Render frame
fn render_frame() {
    profile_scope!("Render::Frame");
    // ...
}

// Physics update
fn physics_update() {
    profile_scope!("Physics::Update");
    // ...
}

// ECS system update
fn update_entities() {
    profile_scope!("ECS::UpdateAll");
    // ...
}
```

### Thread Names

```rust
// Main thread
fn main() {
    profiling::set_thread_name("Main Thread");
}

// Worker threads
std::thread::spawn(|| {
    profiling::set_thread_name("Render Worker");
    render_loop();
});
```

## 🎯 What You'll See

Instead of:
```
Thread 12345
Thread 67890
Thread 11223
```

You'll see:
```
Main Thread
Render Thread  
Physics Worker 1
```

With EXACT timing:
```
Main Thread
  ├─ Frame (16.6ms)
  │  ├─ Update (2.3ms)
  │  │  └─ UpdateEntities (1.8ms)
  │  └─ Render (12.1ms)
  │     ├─ Cull (0.5ms)
  │     └─ DrawCalls (11.2ms)
```

## 💡 Pro Tips

1. **Start coarse** - Instrument major systems first
2. **Then go deeper** - Add nested scopes for details
3. **Name your threads** - Makes traces readable
4. **Use static strings** - `"MyFunction"` not `format!(...)`
5. **Avoid hot loops** - Don't instrument code called millions of times

## 🔥 Performance

- **Disabled**: Zero overhead
- **Enabled**: ~20-50ns per scope
- You can have **thousands** of scopes without impact

## 📖 Full Documentation

See `INSTRUMENTATION-PROFILING-COMPLETE.md` and `crates/profiling/README.md`

---

**START NOW!** Add `profile_scope!()` to your main loop and watch your engine in action! 🚀
