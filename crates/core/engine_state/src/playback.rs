//! Engine-global playback and build state.
//!
//! Simulation speed, multiplayer mode, build configuration and the state of
//! the current play session are properties of the *engine*, not of any one
//! editor. They live in the engine's [`StateStore`](crate::StateStore) as
//! [`PlaybackState`], so the app shell's toolbar can show them without knowing
//! which editors exist; watch it with [`ResourceHandle::changed`].
//!
//! Intent flows the other way, as events: the toolbar publishes
//! `pulsar_events::PlaybackCommand`s on the host bus and the editor that hosts
//! play sessions acts on them and reports back by updating this resource.
//!
//! Several editors can host play sessions at once (one per open level). Only
//! one of them, the *playback host*, reports into [`PlaybackState`]; the
//! others would overwrite its status with their own (#1009). See
//! [`claim_playback_host`].

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{EngineContext, ResourceHandle};

// ── Multiplayer ───────────────────────────────────────────────────────────

/// How a play session participates in the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiplayerMode {
    /// Single player, no networking.
    #[default]
    Standalone,
    /// One player hosts and plays; the others connect to them.
    ListenServer,
    /// A headless authoritative server; every player is a client.
    DedicatedServer,
    /// Join an already-running server.
    Client,
}

impl MultiplayerMode {
    pub const ALL: [Self; 4] = [
        Self::Standalone,
        Self::ListenServer,
        Self::DedicatedServer,
        Self::Client,
    ];

    /// Whether the player count is meaningful in this mode.
    pub fn has_player_count(self) -> bool {
        matches!(self, Self::ListenServer | Self::DedicatedServer)
    }
}

/// Largest player count the quick-config offers.
pub const MAX_PLAYERS: u8 = 16;

/// Quick multiplayer configuration for Play-In-Editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MultiplayerSettings {
    pub mode: MultiplayerMode,
    /// Players in the session (1..=[`MAX_PLAYERS`]): host included for a listen
    /// server, all clients for a dedicated server.
    pub players: u8,
    /// Server simulation rate in Hz.
    pub tick_rate_hz: u16,
    /// Run each player in its own window instead of sharing the editor.
    pub separate_windows: bool,
    /// Artificial one-way latency applied to network traffic, in ms.
    pub latency_ms: u16,
    /// Artificial packet loss, in percent.
    pub packet_loss_pct: u8,
}

impl Default for MultiplayerSettings {
    fn default() -> Self {
        Self {
            mode: MultiplayerMode::Standalone,
            players: 2,
            tick_rate_hz: 60,
            separate_windows: false,
            latency_ms: 0,
            packet_loss_pct: 0,
        }
    }
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
    pub multiplayer: MultiplayerSettings,
    /// A standalone game process launched by Build + Run is alive.
    pub game_running: bool,
    /// A build is running (see `build_config`); the Build button shows Stop.
    pub build_running: bool,
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
            multiplayer: MultiplayerSettings::default(),
            game_running: false,
            build_running: false,
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

/// The standalone game processes started by Build + Run: one per player the
/// multiplayer settings ask for.
///
/// A resource of its own because [`Child`] is neither `Clone` nor `PartialEq`;
/// whether any is alive is mirrored in [`PlaybackState::game_running`].
#[derive(Default)]
pub struct GameProcess(pub Mutex<Vec<Child>>);

impl MultiplayerSettings {
    /// How many copies of the built game a run starts: the player count where
    /// it applies (listen / dedicated server), otherwise one.
    pub fn instance_count(&self) -> u8 {
        if self.mode.has_player_count() {
            self.players.clamp(1, MAX_PLAYERS)
        } else {
            1
        }
    }

    fn normalized(mut self) -> Self {
        self.players = self.players.clamp(1, MAX_PLAYERS);
        self.tick_rate_hz = match self.tick_rate_hz {
            20 | 30 | 60 | 120 => self.tick_rate_hz,
            _ => 60,
        };
        self.latency_ms = self.latency_ms.min(500);
        self.packet_loss_pct = self.packet_loss_pct.min(30);
        self
    }
}

#[derive(Default)]
struct MultiplayerSettingsProject {
    loaded_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct MultiplayerSettingsFile {
    version: u32,
    #[serde(default)]
    settings: MultiplayerSettings,
}

const MULTIPLAYER_SETTINGS_VERSION: u32 = 1;

fn multiplayer_settings_path(project: &Path) -> PathBuf {
    project.join(".pulsar").join("multiplayer_settings.toml")
}

/// Load this project's quick multiplayer setup into the engine-global toolbar
/// state. Missing or malformed files fall back to the built-in defaults.
///
/// Project settings are shared with the project; the file is owned only by
/// this setting and is never used to persist active session state.
pub fn load_multiplayer_settings(project: &Path) {
    let Some(ctx) = EngineContext::global() else {
        return;
    };
    let project_resource = ctx.store.get_or_init::<MultiplayerSettingsProject>();
    if project_resource.read().loaded_path.as_deref() == Some(project) {
        return;
    }

    let path = multiplayer_settings_path(project);
    let settings = match engine_fs::virtual_fs::read_file(&path) {
        Ok(bytes) => match std::str::from_utf8(&bytes)
            .map_err(anyhow::Error::from)
            .and_then(|text| toml::from_str::<MultiplayerSettingsFile>(text).map_err(Into::into))
        {
            Ok(file) if file.version <= MULTIPLAYER_SETTINGS_VERSION => file.settings.normalized(),
            Ok(file) => {
                tracing::warn!(
                    version = file.version,
                    "multiplayer settings were saved by a newer version; using defaults"
                );
                MultiplayerSettings::default()
            }
            Err(error) => {
                tracing::warn!("unreadable multiplayer settings ({error}); using defaults");
                MultiplayerSettings::default()
            }
        },
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            MultiplayerSettings::default()
        }
        Err(error) => {
            tracing::warn!("could not read multiplayer settings: {error}");
            MultiplayerSettings::default()
        }
    };

    playback().update(|state| state.multiplayer = settings);
    project_resource.update(|state| state.loaded_path = Some(project.to_owned()));
}

/// Persist the current quick multiplayer setup for the active project.
pub fn persist_multiplayer_settings() {
    let Some(project) = crate::get_project_path().map(PathBuf::from) else {
        return;
    };
    let file = MultiplayerSettingsFile {
        version: MULTIPLAYER_SETTINGS_VERSION,
        settings: playback().get().multiplayer.normalized(),
    };
    let path = multiplayer_settings_path(&project);
    let text = match toml::to_string_pretty(&file) {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!("could not serialize multiplayer settings: {error}");
            return;
        }
    };
    if let Err(error) = engine_fs::virtual_fs::write_file_atomically(&path, text.as_bytes()) {
        tracing::warn!(path = %path.display(), "could not save multiplayer settings: {error:#}");
    }
}

/// Update the quick multiplayer setup and persist it for the active project.
pub fn update_multiplayer_settings(f: impl FnOnce(&mut MultiplayerSettings)) {
    playback().update(|state| f(&mut state.multiplayer));
    persist_multiplayer_settings();
}

/// The engine's playback state resource.
///
/// # Panics
/// If the engine context is not initialised (it is, before any UI exists).
pub fn playback() -> ResourceHandle<PlaybackState> {
    EngineContext::global()
        .expect("engine initialized")
        .store
        .get_or_init::<PlaybackState>()
}

/// The launched game's process handle resource.
pub fn game_process() -> ResourceHandle<GameProcess> {
    EngineContext::global()
        .expect("engine initialized")
        .store
        .get_or_init::<GameProcess>()
}

/// Set `f` on the playback state, but only publish a change (version bump and
/// listener wake-up) if it actually changed anything, so hosts can report
/// status every tick without waking every watcher.
pub fn update_playback_if_changed(f: impl FnOnce(&mut PlaybackState)) {
    let handle = playback();
    let mut next = handle.get();
    f(&mut next);
    if next != handle.get() {
        handle.set(next);
    }
}

// ── Playback host ──────────────────────────────────────────────────────────

/// The editor that reports into [`PlaybackState`]; 0 when none does.
static PLAYBACK_HOST: AtomicU64 = AtomicU64::new(0);
static NEXT_PLAYBACK_HOST: AtomicU64 = AtomicU64::new(1);

/// A new id for an editor that can host play sessions. Never 0.
pub fn new_playback_host_id() -> u64 {
    NEXT_PLAYBACK_HOST.fetch_add(1, Ordering::Relaxed)
}

/// Make `id` the playback host, e.g. because the user pressed Play in it.
pub fn claim_playback_host(id: u64) {
    PLAYBACK_HOST.store(id, Ordering::Relaxed);
}

/// Make `id` the playback host if no editor is. Returns whether `id` is the
/// host afterwards.
pub fn claim_playback_host_if_free(id: u64) -> bool {
    match PLAYBACK_HOST.compare_exchange(0, id, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => true,
        Err(current) => current == id,
    }
}

/// Whether `id` is the playback host.
pub fn is_playback_host(id: u64) -> bool {
    PLAYBACK_HOST.load(Ordering::Relaxed) == id
}

/// Stop `id` being the playback host. Returns whether it was.
pub fn release_playback_host(id: u64) -> bool {
    PLAYBACK_HOST
        .compare_exchange(id, 0, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_playback_host_at_a_time() {
        let (a, b) = (new_playback_host_id(), new_playback_host_id());
        // Other tests in this binary don't touch the host; start from none.
        PLAYBACK_HOST.store(0, Ordering::Relaxed);

        assert!(claim_playback_host_if_free(a));
        assert!(!claim_playback_host_if_free(b), "a already hosts");
        assert!(claim_playback_host_if_free(a), "a stays host");

        claim_playback_host(b);
        assert!(is_playback_host(b) && !is_playback_host(a));
        assert!(!release_playback_host(a), "a no longer hosts");
        assert!(is_playback_host(b));

        assert!(release_playback_host(b));
        assert!(claim_playback_host_if_free(a), "free again");
        assert!(release_playback_host(a));
    }
}
