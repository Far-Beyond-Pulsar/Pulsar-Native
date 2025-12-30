# Pulsar DAW Engine

An embedded Digital Audio Workstation (DAW) engine for the Pulsar game engine.

## Features

### Audio Core
- **Multi-track mixing** with unlimited audio tracks and aux tracks
- **Real-time audio I/O** using CPAL for cross-platform support (Windows, macOS, Linux)
- **Sample-accurate automation** for volume, pan, and effect parameters
- **Sends and returns** with up to 8 sends per track
- **Master bus** with master volume control
- **Solo and mute** per track with proper solo isolation
- **Track grouping and routing**

### Transport Controls
- Play, pause, stop with sample-accurate positioning
- Loop playback with adjustable loop points
- Seek/scrub functionality
- Metronome/click track with adjustable tempo and time signature
- Real-time position tracking

### Audio Clips
- Drag, drop, and resize clips on timeline
- Trim and fade handles (fade-in/fade-out)
- Crossfade support between overlapping clips
- Multiple audio file format support (WAV, OGG, FLAC)
- Automatic sample rate conversion

### Automation
- Sample-accurate automation curves
- Multiple curve types: Linear, Hold, Bezier
- Automation for volume, pan, send levels, and effect parameters
- Visual automation editor with draw mode
- Quantization and snapping options

### GPU DSP Processing
- **GPU-accelerated convolution** for reverb effects
- **FFT-based EQ** and spectral processing
- **HRTF spatial audio** processing
- **Offline track rendering** to file
- Background compute jobs with progress reporting
- Baked results caching (memory and disk)

### Asset Management
- **Asynchronous loading** of audio files
- **Streaming** of large files to avoid RAM overflow
- **Decoded sample caching** with automatic resampling
- Support for WAV, OGG, and FLAC formats
- Efficient memory management

### GPUI User Interface
Complete interactive UI with:
- **Timeline view** with waveform thumbnails
- **Mixer view** with faders, pan knobs, and meters
- **Automation editor** with curve editing
- **Transport bar** with playback controls
- **Track headers** with mute/solo/record arm buttons
- **Effects rack UI** with parameter controls
- **Project browser** and asset library
- **GPU bake status monitor**

### Project Management
- **JSON-based .pdaw file format** for projects
- **Save/Load** functionality with versioning
- **RON export** option for alternative serialization
- Project validation and integrity checking
- Demo project creation

### ECS Integration
- **Event system** for game engine integration:
  - `PlaySound(track_id)`
  - `StopTrack(track_id)`
  - `SetVolume(track_id, volume)`
  - `SetPan(track_id, pan)`
  - `TriggerAutomation(...)`
  - `Crossfade(...)`
- Low-latency event dispatch using lock-free queues
- Queryable track state for ECS systems

### Performance & Debugging
- **Real-time performance metrics**:
  - Callback duration tracking
  - CPU usage estimation
  - Buffer underrun detection
- **Audio thread latency meter**
- **Per-track CPU/GPU cost estimates**
- **RMS and peak meters** for all tracks and master
- **Debug overlay** with waveform zoom and meters

## Architecture

### Real-Time Audio Thread
- Dedicated real-time thread using CPAL
- Lock-free communication via crossbeam channels
- No allocations or blocking operations on audio thread
- Sample-accurate processing with configurable buffer size

### GPU Compute Pipeline
- WGPU-based compute shaders for heavy DSP work
- Asynchronous job scheduling
- Progress reporting and result validation
- Automatic fallback to CPU if GPU unavailable

### Asset Loading
- Tokio-based async runtime for I/O operations
- Parallel decoding of multiple files
- Smart caching with path and sample-rate keys
- Automatic resampling for sample rate mismatch

### UI Architecture
- GPUI component-based UI
- Reactive state management
- Panel system integration
- Tab management for multiple projects

## File Format

Projects are saved as `.pdaw` files in JSON format:

```json
{
  "version": 1,
  "name": "My Project",
  "sample_rate": 48000.0,
  "tracks": [...],
  "transport": {...},
  "master_track": {...}
}
```

## Usage

### Opening a DAW Project

1. Navigate to your project in the file explorer
2. Double-click a `.pdaw` file
3. The DAW editor opens in a new tab

### Creating a New Project

```rust
use pulsar_engine::ui::panels::daw_editor::{DawProject, create_demo_project};

// Create a new empty project
let project = DawProject::new("My Music");

// Or create a demo project with example tracks
let project = create_demo_project();

// Save the project
project.save("my_music.pdaw")?;
```

### Using the Audio Service

```rust
use pulsar_engine::ui::panels::daw_editor::{AudioService, Track, TrackType};

// Initialize the audio service
let service = AudioService::new().await?;

// Add a track
let track = Track::new("Drums", TrackType::Audio);
let track_id = service.add_track(track).await;

// Control playback
service.play().await?;
service.pause().await?;
service.stop().await?;

// Adjust track parameters
service.set_track_volume(track_id, 0.8).await?;
service.set_track_pan(track_id, -0.3).await?;
service.set_track_mute(track_id, true).await?;
```

### ECS Integration

```rust
use pulsar_engine::ui::panels::daw_editor::{
    AudioService, EcsAudioBridge, AudioEvent
};
use std::sync::Arc;

// Create the bridge
let service = Arc::new(AudioService::new().await?);
let bridge = EcsAudioBridge::new(service);

// Dispatch events from your game systems
bridge.dispatch_event(AudioEvent::PlaySound { track_id }).await;
bridge.dispatch_event(AudioEvent::SetVolume { 
    track_id, 
    volume: 0.5 
}).await;

// Query track state
if let Some(state) = bridge.get_track_state(track_id).await {
    println!("Track '{}' volume: {}", state.name, state.volume);
    println!("Peak level: {} dB", state.meter.peak_left);
}
```

## System Requirements

- **Sample Rate**: 48000 Hz (configurable)
- **Buffer Size**: 512 samples (configurable)
- **Max Tracks**: 128
- **Max Sends per Track**: 8

### Platform Support
- Windows (WASAPI)
- macOS (CoreAudio)
- Linux (ALSA, PulseAudio, JACK)

### GPU Requirements
- Vulkan, Metal, or DirectX 12 support for GPU DSP features
- Graceful fallback to CPU if GPU unavailable

## Dependencies

### Audio
- `cpal` - Cross-platform audio I/O
- `hound` - WAV file decoding
- `lewton` - OGG Vorbis decoding
- `claxon` - FLAC decoding

### Concurrency
- `tokio` - Async runtime
- `crossbeam` - Lock-free channels
- `parking_lot` - Efficient locks
- `dashmap` - Concurrent hashmap

### GPU
- `wgpu` - GPU compute pipeline
- `bytemuck` - Safe byte casting

### Serialization
- `serde` - Serialization framework
- `serde_json` - JSON format
- `ron` - RON format

### Utilities
- `uuid` - Unique identifiers
- `chrono` - Timestamps
- `anyhow` - Error handling

## Testing

The DAW engine includes comprehensive tests:

```bash
# Run all tests
cargo test --package pulsar_engine

# Run DAW-specific tests
cargo test --package pulsar_engine daw_editor

# Run with audio device (may require hardware)
cargo test --package pulsar_engine --features=audio-tests
```

## Performance Considerations

### Real-Time Thread
- Runs at high priority (platform-dependent)
- No allocations during audio callback
- Lock-free communication only
- Callback duration typically < 1ms for 512 sample buffer

### GPU Compute
- Large convolutions: ~5-10ms for 2-second IR
- FFT operations: ~1-2ms for 8192 point FFT
- Results cached to avoid redundant computation

### Memory Usage
- Base engine: ~50MB
- Per track: ~500KB
- Cached audio: Varies by file size
- GPU buffers: ~10MB per active job

## Known Limitations

1. **Maximum tracks**: Hard limit of 128 tracks
2. **Send routing**: Maximum 8 sends per track
3. **Automation**: Linear/Hold/Bezier curves only
4. **File formats**: WAV, OGG, FLAC only (no MP3)
5. **GPU features**: Require compatible GPU (fallback to CPU)

## Future Enhancements

- VST/AU plugin hosting
- MIDI support with virtual instruments
- Spectral editing and analysis
- Time-stretching and pitch-shifting
- Multi-channel surround support
- Network collaboration features

## License

This code is part of the Pulsar game engine and follows the same license.

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────┐
│                     GPUI UI Layer                        │
│  ┌──────────┐  ┌──────────┐  ┌────────────────────┐    │
│  │ Timeline │  │  Mixer   │  │ Automation Editor  │    │
│  └──────────┘  └──────────┘  └────────────────────┘    │
└─────────────────────┬───────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────┐
│                  Audio Service                           │
│  ┌────────────┐  ┌──────────────┐  ┌────────────────┐  │
│  │ Track Mgmt │  │  Transport   │  │ Asset Manager  │  │
│  └────────────┘  └──────────────┘  └────────────────┘  │
└─────────┬─────────────────────┬─────────────────────────┘
          │                     │
          ▼                     ▼
┌──────────────────┐   ┌──────────────────┐
│  Audio Graph     │   │    GPU DSP       │
│  ┌────────────┐  │   │  ┌────────────┐  │
│  │ Mixing     │  │   │  │ Convolution│  │
│  │ Routing    │  │   │  │ FFT        │  │
│  │ Effects    │  │   │  │ HRTF       │  │
│  └────────────┘  │   │  └────────────┘  │
└────────┬─────────┘   └──────────────────┘
         │
         ▼
┌─────────────────────────────────────┐
│   Real-Time Audio Thread (CPAL)    │
│   ┌─────────────────────────────┐  │
│   │  Lock-Free Command Queue    │  │
│   │  Sample Processing Loop     │  │
│   │  Audio Device Output        │  │
│   └─────────────────────────────┘  │
└─────────────────────────────────────┘
```
