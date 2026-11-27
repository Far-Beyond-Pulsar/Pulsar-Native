//! The game Play is running, however it is hosted: in the editor's own
//! process ([`InEditorGame`], a project whose gameplay is scripts) or as the
//! project's compiled library ([`PieHost`], native gameplay).
//!
//! A panic of an in-editor game stops it and is reported; it never unwinds
//! into the editor (the library form catches its panics at its boundary).

use std::panic::{catch_unwind, AssertUnwindSafe};

use engine_backend::services::PieHost;
use pulsar_events::gamma::ffi::ForeignBus;
use pulsar_game::embed::InEditorGame;
use pulsar_pie_abi::{control, InputEvent};

pub(super) enum RunningGame {
    Library(PieHost),
    Editor(Box<EditorHosted>),
}

/// An in-editor game with its event hub published for editor plugins.
pub(super) struct EditorHosted {
    /// `None` once it panicked.
    game: Option<InEditorGame>,
    session_bus: Option<ForeignBus>,
}

impl EditorHosted {
    pub(super) fn new(game: InEditorGame) -> Self {
        // SAFETY: a fresh export of the game's hub, whose reference is ours.
        let session_bus = match unsafe { ForeignBus::from_raw(game.export_event_bus()) } {
            Ok(bus) => {
                pulsar_events::announce_session_started(&bus);
                Some(bus)
            }
            Err(error) => {
                tracing::warn!("PiE: the game's event bus was not published: {error:?}");
                None
            }
        };
        Self {
            game: Some(game),
            session_bus,
        }
    }

    /// Run `work` on the game; a panic ends the game.
    fn with<T>(&mut self, what: &str, work: impl FnOnce(&mut InEditorGame) -> T) -> Option<T> {
        let game = self.game.as_mut()?;
        match catch_unwind(AssertUnwindSafe(|| work(game))) {
            Ok(value) => Some(value),
            Err(_) => {
                tracing::error!("PiE: the game panicked during {what}; it is stopped");
                self.stop();
                None
            }
        }
    }

    fn stop(&mut self) {
        // Plugins drop their handles onto the game's hub first.
        if let Some(bus) = self.session_bus.take() {
            pulsar_events::announce_session_stopping();
            drop(bus);
        }
        if let Some(game) = self.game.take() {
            if catch_unwind(AssertUnwindSafe(|| drop(game))).is_err() {
                tracing::error!("PiE: the game panicked while shutting down");
            }
        }
    }
}

impl Drop for EditorHosted {
    fn drop(&mut self) {
        self.stop();
    }
}

impl RunningGame {
    pub(super) fn is_in_editor(&self) -> bool {
        matches!(self, Self::Editor(_))
    }

    pub(super) fn stop(&mut self) {
        match self {
            Self::Library(host) => host.stop(),
            Self::Editor(host) => host.stop(),
        }
    }

    pub(super) fn tick(&mut self, dt: f32) {
        match self {
            Self::Library(host) => host.tick(dt),
            // The game's tick loop keeps its own clock.
            Self::Editor(host) => {
                host.with("a frame", InEditorGame::tick);
            }
        }
    }

    pub(super) fn resize(&mut self, width: u32, height: u32) {
        match self {
            Self::Library(host) => host.resize(width, height),
            Self::Editor(host) => {
                host.with("a resize", |game| game.resize(width, height));
            }
        }
    }

    pub(super) fn input(&mut self, event: &InputEvent) {
        match self {
            Self::Library(host) => host.input(event),
            Self::Editor(host) => {
                host.with("input", |game| game.input(event));
            }
        }
    }

    /// Forward an asset change. An in-editor game hears the editor's asset
    /// bus itself (forwarding would publish it there again, which the editor
    /// would forward again).
    pub(super) fn asset_updated(&mut self, event: &plugin_editor_api::AssetUpdated) {
        if let Self::Library(host) = self {
            host.asset_updated(event);
        }
    }

    pub(super) fn pause(&mut self) -> bool {
        match self {
            Self::Library(host) => host.pause(),
            Self::Editor(host) => host
                .with("pause", |game| game.control(control::PAUSE, 0) == 1)
                .unwrap_or(false),
        }
    }

    pub(super) fn resume(&mut self) -> bool {
        match self {
            Self::Library(host) => host.resume(),
            Self::Editor(host) => host
                .with("resume", |game| game.control(control::RESUME, 0) == 1)
                .unwrap_or(false),
        }
    }

    pub(super) fn step(&mut self, frames: u32) -> bool {
        match self {
            Self::Library(host) => host.step(frames),
            Self::Editor(host) => host
                .with("step", |game| {
                    game.control(control::STEP, u64::from(frames)) == 1
                })
                .unwrap_or(false),
        }
    }

    pub(super) fn is_paused(&self) -> Option<bool> {
        match self {
            Self::Library(host) => host.is_paused(),
            Self::Editor(host) => host.game.as_ref().map(InEditorGame::is_paused),
        }
    }

    /// Whether the game asked to quit, or can no longer run.
    pub(super) fn quit_requested(&self) -> bool {
        match self {
            Self::Library(host) => host.quit_requested(),
            Self::Editor(host) => host
                .game
                .as_ref()
                .map_or(true, InEditorGame::quit_requested),
        }
    }

    pub(super) fn has_simulation_control(&self) -> bool {
        match self {
            Self::Library(host) => host.has_simulation_control(),
            Self::Editor(_) => true,
        }
    }

    pub(super) fn take_problems(&mut self) -> Vec<pulsar_events::ScriptProblem> {
        match self {
            Self::Library(host) => host.take_problems(),
            Self::Editor(host) => host
                .with("reporting problems", InEditorGame::take_problems)
                .unwrap_or_default(),
        }
    }

    pub(super) fn events_snapshot(&self) -> Option<pulsar_events::EventsSnapshot> {
        match self {
            Self::Library(host) => host.events_snapshot(),
            Self::Editor(host) => host.game.as_ref().map(InEditorGame::events_snapshot),
        }
    }

    /// The texture the game draws into, for this frame's blit.
    pub(super) fn out_texture(&self) -> Option<&wgpu::Texture> {
        match self {
            // SAFETY: the texture is memory of the loaded library, which
            // stays mapped while `self` is borrowed (`stop` and drop need
            // `&mut` or ownership).
            Self::Library(host) => unsafe { host.out_texture() },
            Self::Editor(host) => host.game.as_ref().map(InEditorGame::out_texture),
        }
    }
}
