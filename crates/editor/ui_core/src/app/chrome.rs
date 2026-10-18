//! Window chrome: the file manager drawer and the status-bar window toggles.
//!
//! One owner for what used to be loose fields on `AppState`.
//!
//! **Persisted** (in the layout's `chrome` block, see [`SavedChrome`]):
//! - `docked`: whether the file manager is docked under the editor instead of
//!   sliding over it. It is a deliberate arrangement the user chose.
//! - `height`: the drawer's height, shared by the docked and overlay forms.
//!
//! **Not persisted**, on purpose:
//! - `drawer_open`: the hover / click overlay form of the drawer. It is a
//!   momentary peek; restoring it would greet the user with an open overlay.
//! - `resizing` / `resize_start_*` / `suppress_for_drag`: mid-gesture state.
//! - `mission_control_open`, `git_manager_open`: these mirror *separate OS
//!   windows* opened through the window registry; restoring the flag would
//!   light the status-bar button with no window behind it (and
//!   `git_manager_open` is never cleared when its window closes, so a saved
//!   value would be stale after first use). Reopening those windows at startup
//!   is a behaviour change this layout work does not make.
//! - The problems and type-debugger drawers have no inline state: they open as
//!   their own windows through the registry, so there is nothing to save.
//!
//! Every mutation of persisted chrome must be followed by
//! [`PulsarApp::chrome_changed`], because these paths emit no `DockEvent` and
//! would otherwise never arm the debounced layout save.

use gpui::Context;
use serde::{Deserialize, Serialize};

use super::PulsarApp;

/// Height the drawer starts at.
pub const DEFAULT_DRAWER_HEIGHT: f32 = 400.0;
/// Bounds the drawer can be resized to.
pub const MIN_DRAWER_HEIGHT: f32 = 200.0;
pub const MAX_DRAWER_HEIGHT: f32 = 700.0;

/// Live chrome state of one editor window.
#[derive(Clone, Debug)]
pub struct ChromeState {
    // ── persisted ──
    /// The file manager is docked under the editor.
    pub drawer_docked: bool,
    /// Drawer height in pixels, within `MIN..=MAX_DRAWER_HEIGHT`.
    pub drawer_height: f32,
    // ── transient ──
    /// The folder the drawer shows, mirrored from it so the layout can save
    /// it (the drawer owns the real value). The layout writes it as a
    /// project-relative path, see [`SavedChrome::file_manager_folder`].
    pub file_manager_folder: Option<std::path::PathBuf>,
    /// The overlay form of the drawer is showing (only while not docked).
    pub drawer_open: bool,
    pub drawer_resizing: bool,
    pub drawer_resize_start_y: f32,
    pub drawer_resize_start_height: f32,
    /// Auto-close the drawer during an asset drag.
    pub suppress_drawer_for_drag: bool,
    /// The Mission Control window is open.
    pub mission_control_open: bool,
    /// The Git Manager window has been opened.
    pub git_manager_open: bool,
}

impl Default for ChromeState {
    fn default() -> Self {
        Self {
            drawer_docked: false,
            drawer_height: DEFAULT_DRAWER_HEIGHT,
            file_manager_folder: None,
            drawer_open: false,
            drawer_resizing: false,
            drawer_resize_start_y: 0.0,
            drawer_resize_start_height: DEFAULT_DRAWER_HEIGHT,
            suppress_drawer_for_drag: false,
            mission_control_open: false,
            git_manager_open: false,
        }
    }
}

/// What a window saves about its chrome, in the layout's `chrome` block.
/// Missing fields take their defaults; defaults are not written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedChrome {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub file_manager_docked: bool,
    #[serde(default = "default_height", skip_serializing_if = "is_default_height")]
    pub file_manager_height: f32,
    /// The folder the drawer was showing, relative to the project root with
    /// forward slashes, so it reopens where the user left it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_manager_folder: Option<String>,
}

fn default_height() -> f32 {
    DEFAULT_DRAWER_HEIGHT
}

fn is_default_height(height: &f32) -> bool {
    *height == DEFAULT_DRAWER_HEIGHT
}

impl Default for SavedChrome {
    fn default() -> Self {
        Self {
            file_manager_docked: false,
            file_manager_height: DEFAULT_DRAWER_HEIGHT,
            file_manager_folder: None,
        }
    }
}

impl SavedChrome {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Clamp a saved height the way resizing does; a non-finite value (a
    /// hand-edited file) falls back to the default.
    fn sanitized_height(&self) -> f32 {
        if self.file_manager_height.is_finite() {
            self.file_manager_height
                .clamp(MIN_DRAWER_HEIGHT, MAX_DRAWER_HEIGHT)
        } else {
            DEFAULT_DRAWER_HEIGHT
        }
    }
}

impl ChromeState {
    /// The persisted part of the chrome.
    pub fn save(&self) -> SavedChrome {
        SavedChrome {
            file_manager_docked: self.drawer_docked,
            file_manager_height: self.drawer_height,
            // Relative to the project root, which only the layout knows.
            file_manager_folder: None,
        }
    }

    /// Apply a saved chrome. Rendering reads these fields directly, so a
    /// restored `docked` shows exactly what the dock button produces (that
    /// handler only sets the flags too). The overlay is closed.
    pub fn restore(&mut self, saved: &SavedChrome) {
        self.drawer_docked = saved.file_manager_docked;
        self.drawer_height = saved.sanitized_height();
        self.drawer_open = false;
    }

    /// Clamp a height produced by a resize drag.
    pub fn clamp_height(height: f32) -> f32 {
        height.clamp(MIN_DRAWER_HEIGHT, MAX_DRAWER_HEIGHT)
    }
}

impl PulsarApp {
    /// Persisted chrome changed: arm the debounced layout save.
    pub(super) fn chrome_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_layout_save(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_chrome_serialises_to_nothing() {
        let saved = ChromeState::default().save();
        assert!(saved.is_empty());
        assert_eq!(serde_json::to_string(&saved).unwrap(), "{}");
    }

    #[test]
    fn chrome_round_trips() {
        let mut live = ChromeState::default();
        live.drawer_docked = true;
        live.drawer_height = 550.0;
        let json = serde_json::to_string(&live.save()).unwrap();
        let saved: SavedChrome = serde_json::from_str(&json).unwrap();
        let mut restored = ChromeState::default();
        restored.drawer_open = true;
        restored.restore(&saved);
        assert!(restored.drawer_docked);
        assert_eq!(restored.drawer_height, 550.0);
        assert!(!restored.drawer_open, "the overlay never restores open");
    }

    #[test]
    fn restore_clamps_and_rejects_garbage_heights() {
        let mut live = ChromeState::default();
        for (saved, expected) in [
            (10.0, MIN_DRAWER_HEIGHT),
            (9000.0, MAX_DRAWER_HEIGHT),
            (f32::NAN, DEFAULT_DRAWER_HEIGHT),
            (f32::INFINITY, DEFAULT_DRAWER_HEIGHT),
        ] {
            live.restore(&SavedChrome {
                file_manager_docked: false,
                file_manager_height: saved,
                file_manager_folder: None,
            });
            assert_eq!(live.drawer_height, expected, "saved {saved}");
        }
    }

    #[test]
    fn missing_fields_take_defaults() {
        let saved: SavedChrome = serde_json::from_str(r#"{"file_manager_docked":true}"#).unwrap();
        assert!(saved.file_manager_docked);
        assert_eq!(saved.file_manager_height, DEFAULT_DRAWER_HEIGHT);
        let _: SavedChrome = serde_json::from_str("{}").unwrap();
    }

    #[test]
    fn transient_state_is_not_saved() {
        let mut live = ChromeState::default();
        live.drawer_open = true;
        live.mission_control_open = true;
        live.git_manager_open = true;
        live.suppress_drawer_for_drag = true;
        assert!(live.save().is_empty());
    }
}
