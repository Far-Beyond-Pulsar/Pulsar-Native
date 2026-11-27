//! What the running game asks of its host (the window, Play-in-Editor, the
//! headless runner), and what the host needs to know about the level.
//!
//! The tick loop runs on its own thread in the windowed game, so the host
//! reads this shared state instead of the loop: clone [`GameControl`] before
//! the loop moves to its thread (`TickLoop::control`). Scripts reach it
//! through the `game::` natives (`scripting::game_natives`), applied by the
//! tick loop after the script phase.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use engine_backend::scene::EditorCamera;

/// Who the player's devices belong to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    /// Gameplay: a click captures the cursor, and gameplay input (look, move,
    /// actions, the free camera) is read.
    #[default]
    Game,
    /// A menu: the cursor is shown and free, a click does not capture it,
    /// gameplay input is not read, and widgets get the pointer. Scripts still
    /// hear raw key events (a menu's Escape).
    Ui,
}

impl InputMode {
    /// `Game` or `UI`, in any case; `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "game" => Some(Self::Game),
            "ui" => Some(Self::Ui),
            _ => None,
        }
    }
}

/// One request of a script to the host, applied after the script phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostRequest {
    /// Switch to this level (project-relative).
    LoadLevel(String),
    SetInputMode(InputMode),
    /// End the game.
    Quit,
}

/// What happened to a level, for the host to follow.
#[derive(Clone, Debug)]
pub enum LevelEvent {
    /// The old level (if any) ended and `level` is being hydrated. Its render
    /// pipeline is selected (`pipelines::level_pipeline`).
    Begun { level: String },
    /// `level` is in the world and gameplay ticks again.
    Loaded {
        level: String,
        /// The editor camera saved in the file: where a free camera starts.
        editor_camera: Option<EditorCamera>,
    },
    /// `level` could not be loaded.
    Failed { level: String, error: String },
}

/// Level events kept for a host that is not reading them (a headless run).
const MAX_KEPT_EVENTS: usize = 64;

#[derive(Default)]
struct State {
    input_mode: AtomicU8,
    quit: AtomicBool,
    levels: Mutex<VecDeque<LevelEvent>>,
}

/// The shared state; see the module doc.
#[derive(Clone, Default)]
pub struct GameControl(Arc<State>);

impl GameControl {
    pub fn input_mode(&self) -> InputMode {
        match self.0.input_mode.load(Ordering::SeqCst) {
            0 => InputMode::Game,
            _ => InputMode::Ui,
        }
    }

    pub fn set_input_mode(&self, mode: InputMode) {
        self.0
            .input_mode
            .store(u8::from(mode == InputMode::Ui), Ordering::SeqCst);
    }

    /// Whether the game asked to end.
    pub fn quit_requested(&self) -> bool {
        self.0.quit.load(Ordering::SeqCst)
    }

    pub fn request_quit(&self) {
        self.0.quit.store(true, Ordering::SeqCst);
    }

    fn levels(&self) -> MutexGuard<'_, VecDeque<LevelEvent>> {
        self.0
            .levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn publish_level(&self, event: LevelEvent) {
        let mut levels = self.levels();
        if levels.len() >= MAX_KEPT_EVENTS {
            levels.pop_front();
        }
        levels.push_back(event);
    }

    /// The level events since the last call, in order (one reader: the host).
    pub fn take_level_events(&self) -> Vec<LevelEvent> {
        self.levels().drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_modes_parse_in_any_case() {
        assert_eq!(InputMode::parse("UI"), Some(InputMode::Ui));
        assert_eq!(InputMode::parse(" game "), Some(InputMode::Game));
        assert_eq!(InputMode::parse("menu"), None);
    }
}
