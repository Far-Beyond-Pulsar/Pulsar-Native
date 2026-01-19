# DTrace Profiler

Cross-platform Rust wrapper for DTrace CPU profiling with real-time flamegraph generation.

## Supported Platforms

- **Windows**: DTrace on Windows (Windows 10 Build 18980+, Server 2019+)
- **macOS**: Native DTrace support (built-in)
- **Linux**: DTrace for Linux (requires installation)
- **FreeBSD**: Native DTrace support (built-in)

## Prerequisites

### Windows
DTrace must be installed from Windows Optional Features:

#### Option 1: Via Settings UI
1. Open Settings → Apps → Optional Features
2. Click "Add a feature"
3. Search for "DTrace" and install

#### Option 2: Via PowerShell (Run as Administrator)
```powershell
Enable-WindowsOptionalFeature -Online -FeatureName DTrace
```

#### Option 3: Via DISM (Run as Administrator)
```cmd
dism /online /enable-feature /featurename:DTrace /all
```

After installation, reboot may be required. Verify with:
```cmd
dtrace -V
```

### macOS
DTrace is built-in, but requires elevated privileges:
```bash
# Run your application with sudo
sudo ./your_app

# Or, for development:
sudo cargo run
```

### Linux
Install DTrace from your distribution:

**Ubuntu/Debian:**
```bash
sudo apt-get install systemtap-sdt-dev
# Or build from source: https://github.com/opendtrace/linux
```

**Fedora/RHEL:**
```bash
sudo dnf install dtrace
```

**Arch:**
```bash
yay -S dtrace
```

### FreeBSD
DTrace is built-in. Ensure your user has permissions:
```bash
sudo sysctl security.bsd.unprivileged_proc_debug=1
```

## Usage

```rust
use dtrace_profiler::DTraceProfiler;

let profiler = DTraceProfiler::new();

// Start profiling at 99 Hz (99 samples per second)
profiler.start(99)?;

// ... your application runs ...

// Periodically get samples
let samples = profiler.take_samples();
for sample in samples {
    println!("Thread {}: {} frames", sample.thread_id, sample.stack_frames.len());
    for frame in &sample.stack_frames {
        println!("  {}::{} @ 0x{:x}", 
            frame.module_name, 
            frame.function_name, 
            frame.address);
    }
}

// Stop profiling
profiler.stop();
```

## Output Format

Each `Sample` contains:
- `thread_id`: Thread ID that was sampled
- `process_id`: Process ID (always the current process)
- `timestamp_ns`: Timestamp in nanoseconds since UNIX epoch
- `stack_frames`: Vector of `StackFrame` (bottom to top):
  - `function_name`: Symbol or function name
  - `module_name`: Library/module name (e.g., "kernel", "libsystem_kernel.dylib")
  - `address`: Memory address or offset

## Implementation Details

This crate:
1. Spawns a `dtrace` subprocess with a custom profiling script
2. Samples CPU at the specified frequency (default 99 Hz)
3. Captures user-space stack traces with `ustack(20)` (up to 20 frames)
4. Parses the output into structured `Sample` objects
5. Runs in a background thread without blocking your application

### DTrace Script

The profiler uses this D script:
```d
profile-99 /pid == $PID/
{
    @stacks[tid, ustack(20)] = count();
}

tick-1sec
{
    printa("TID:%d COUNT:%@u\n%k\n", @stacks);
    clear(@stacks);
}
```

### Permissions

DTrace requires elevated privileges on most platforms:
- **Windows**: Run as Administrator
- **macOS/Linux/FreeBSD**: Run with `sudo` or configure appropriate capabilities

## Troubleshooting

### "dtrace: failed to initialize dtrace: DTrace requires additional privileges"
- Windows: Run as Administrator
- Unix: Run with `sudo` or configure privileges

### "dtrace: command not found"
- Install DTrace following the prerequisites above

### Empty samples returned
- Ensure your application is actually running CPU-intensive work
- Try increasing the sample frequency (e.g., 997 Hz)
- Check DTrace stderr output for errors

## Performance Impact

At 99 Hz sampling:
- **Overhead**: < 1% CPU typically
- **Memory**: Minimal (samples are collected and cleared periodically)
- **Latency**: No impact on application latency (samples in separate process)

Higher frequencies (e.g., 997 Hz) increase overhead proportionally.

