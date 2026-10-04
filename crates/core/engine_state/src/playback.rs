//! Engine-global playback and build state.
//!
//! Play / pause / step / stop, simulation speed, multiplayer mode and the build
//! configuration are properties of the *engine*, not of any one editor. They
//! live here, in one process-wide store, so the app shell's toolbar can show
//! and drive them without knowing which editors exist.
//!
//! Editors take part as **playback hosts**: a host calls
//! [`Playback::register_host`], services the [`PlaybackCommand`]s the toolbar
//! sends ([`Playback::drain_commands`]) and mirrors what it is doing back into
//! [`PlaybackState`] ([`Playback::update`]). The toolbar never touches an
//! editor; with no host registered, [`Playback::send`] reports that nothing
//! will act on the command.

use std::collections::VecDeque;
use std::process::Child;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use parking_lot::{Mutex, RwLock};

// ── Multiplayer Mode ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiplayerMode {
    Offline,
    Host,
    Client,
}

// ── Build Configuration ───────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildConfig {
    Debug,
    Release,
    Shipping,
}

/// Which action the Build button's primary click performs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BuildMode {
    #[default]
    Build,
    BuildAndRun,
    Check,
    Update,
    UpdateBuildAndRun,
    BuildScratch,
    BuildAndRunScratch,
    CheckScratch,
}

/// Complete Rust target platform and architecture support (excluding WASM).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetPlatform {
    WindowsX86_64Msvc,
    WindowsI686Msvc,
    WindowsAarch64Msvc,
    WindowsX86_64Gnu,
    WindowsI686Gnu,
    LinuxX86_64Gnu,
    LinuxI686Gnu,
    LinuxAarch64Gnu,
    LinuxArmv7Gnueabihf,
    LinuxArmGnueabi,
    LinuxArmGnueabihf,
    LinuxMips64Gnuabi64,
    LinuxMips64elGnuabi64,
    LinuxMipsGnu,
    LinuxMipselGnu,
    LinuxPowerpc64Gnu,
    LinuxPowerpc64leGnu,
    LinuxPowerpcGnu,
    LinuxRiscv64Gc,
    LinuxS390xGnu,
    LinuxSparcv9,
    LinuxX86_64Musl,
    LinuxAarch64Musl,
    LinuxArmv7Musleabihf,
    LinuxMipselMusl,
    LinuxMipsMusl,
    MacOsX86_64,
    MacOsAarch64,
    IosAarch64,
    IosX86_64,
    IosAarch64Sim,
    AndroidAarch64,
    AndroidArmv7,
    AndroidI686,
    AndroidX86_64,
    FreeBsdX86_64,
    FreeBsdI686,
    NetBsdX86_64,
    OpenBsdX86_64,
    DragonFlyX86_64,
    SolarisSparcv9,
    SolarisX86_64,
    IlumosX86_64,
    RedoxX86_64,
    FuchsiaAarch64,
    FuchsiaX86_64,
    PlayStationPs4,
    PlayStationPs5,
    XboxOne,
    XboxSeriesXS,
    NintendoSwitch,
}

// ── Playback state ─────────────────────────────────────────────────────────

/// Where a play session currently is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PlayPhase {
    /// Editing; nothing is running.
    #[default]
    Stopped,
    /// Play was requested and the game is still being built / loaded.
    Building,
    /// A play session is active (possibly paused).
    Playing,
}

/// Everything the global toolbar shows. `Clone + PartialEq` so views can
/// compare snapshots to decide whether to redraw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackState {
    /// Simulation time scale (1.0 = real time).
    pub time_scale: f32,
    /// Target frame rate for the game loop (0 = uncapped).
    pub target_fps: u32,
    pub multiplayer_mode: MultiplayerMode,
    pub build_config: BuildConfig,
    pub target_platform: TargetPlatform,
    pub build_mode: BuildMode,
    /// A standalone game process launched by Build + Run is alive.
    pub game_running: bool,
    pub phase: PlayPhase,
    pub paused: bool,
    /// The running game supports pause / step.
    pub supports_control: bool,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            time_scale: 1.0,
            target_fps: 60,
            multiplayer_mode: MultiplayerMode::Offline,
            build_config: BuildConfig::Debug,
            target_platform: TargetPlatform::WindowsX86_64Msvc,
            build_mode: BuildMode::Build,
            game_running: false,
            phase: PlayPhase::Stopped,
            paused: false,
            supports_control: false,
        }
    }
}

impl PlaybackState {
    pub fn is_stopped(&self) -> bool {
        self.phase == PlayPhase::Stopped
    }
}

/// A request from the toolbar (or anything else) to the playback host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackCommand {
    /// Start playing, or hot-reload if already playing.
    Play,
    Stop,
    /// Pause if running, resume if paused.
    TogglePause,
    /// Advance one frame while paused.
    Step,
}

/// The process-wide playback store. Get it with [`playback`].
pub struct Playback {
    state: RwLock<PlaybackState>,
    commands: Mutex<VecDeque<PlaybackCommand>>,
    hosts: AtomicUsize,
    game_process: Mutex<Option<Child>>,
}

static PLAYBACK: OnceLock<Playback> = OnceLock::new();

/// The engine's playback store.
pub fn playback() -> &'static Playback {
    PLAYBACK.get_or_init(|| Playback {
        state: RwLock::new(PlaybackState::default()),
        commands: Mutex::new(VecDeque::new()),
        hosts: AtomicUsize::new(0),
        game_process: Mutex::new(None),
    })
}

impl Playback {
    pub fn state(&self) -> PlaybackState {
        *self.state.read()
    }

    /// Mutate the state. Hosts should prefer writing only on change, so
    /// watchers comparing snapshots stay quiet.
    pub fn update<R>(&self, f: impl FnOnce(&mut PlaybackState) -> R) -> R {
        f(&mut self.state.write())
    }

    /// The standalone game process started by Build + Run.
    pub fn game_process(&self) -> &Mutex<Option<Child>> {
        &self.game_process
    }

    /// Queue `command` for a host. Returns `false` (and queues nothing) when
    /// no host is registered to act on it.
    pub fn send(&self, command: PlaybackCommand) -> bool {
        if self.hosts.load(Ordering::Acquire) == 0 {
            return false;
        }
        self.commands.lock().push_back(command);
        true
    }

    /// Take every queued command. Called by hosts.
    pub fn drain_commands(&self) -> Vec<PlaybackCommand> {
        let mut queue = self.commands.lock();
        if queue.is_empty() {
            return Vec::new();
        }
        queue.drain(..).collect()
    }

    /// Whether any host is currently registered.
    pub fn has_host(&self) -> bool {
        self.hosts.load(Ordering::Acquire) > 0
    }

    /// Register as a playback host for as long as the guard lives.
    pub fn register_host(&'static self) -> PlaybackHost {
        self.hosts.fetch_add(1, Ordering::AcqRel);
        PlaybackHost { playback: self }
    }
}

/// Keeps a host registered; unregisters on drop.
pub struct PlaybackHost {
    playback: &'static Playback,
}

impl Drop for PlaybackHost {
    fn drop(&mut self) {
        if self.playback.hosts.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last host gone: nobody will service what is queued, and a stale
            // command must not fire when the next host appears.
            self.playback.commands.lock().clear();
            self.playback.update(|s| {
                s.phase = PlayPhase::Stopped;
                s.paused = false;
                s.supports_control = false;
            });
        }
    }
}
