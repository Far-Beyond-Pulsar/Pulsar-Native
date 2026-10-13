//! Saved window geometry: size, position and maximized / fullscreen state.
//!
//! Shared by the main window and, in a later step, pop-out windows.

use std::path::Path;

use gpui::{Bounds, Pixels, WindowBounds};
use serde::{Deserialize, Serialize};

use super::format::{layout_path, read_slot, PRIMARY_SLOT};

/// How the window was shown. The bounds saved alongside are the *restore*
/// bounds, so a maximized window comes back maximized and un-maximizes to its
/// previous size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum SavedWindowState {
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedWindow {
    pub state: SavedWindowState,
    pub bounds: Bounds<Pixels>,
}

/// Smallest saved window side we trust, in logical pixels.
const MIN_WINDOW_SIDE: f32 = 320.0;

impl SavedWindow {
    /// Capture `bounds`. For a maximized or fullscreen window the platform
    /// reports the screen-sized bounds, so `restore` (the last windowed
    /// bounds) is saved instead; leaving full screen then returns to the size
    /// the window had before.
    pub fn capture(bounds: WindowBounds, restore: Option<Bounds<Pixels>>) -> Self {
        let (state, bounds) = match bounds {
            WindowBounds::Windowed(b) => (SavedWindowState::Windowed, b),
            WindowBounds::Maximized(b) => (SavedWindowState::Maximized, restore.unwrap_or(b)),
            WindowBounds::Fullscreen(b) => (SavedWindowState::Fullscreen, restore.unwrap_or(b)),
        };
        Self { state, bounds }
    }

    /// The bounds to open a window with. A windowed window needs a plausible
    /// saved size, else `None` (use the default). A maximized or fullscreen
    /// window does not care about its size, so it is reopened that way even if
    /// the saved size is unusable; the bounds then only say which display and
    /// what size to leave it at.
    pub fn to_window_bounds(self) -> Option<WindowBounds> {
        let size = self.bounds.size;
        let usable = |v: Pixels| f32::from(v).is_finite() && f32::from(v) >= MIN_WINDOW_SIDE;
        let sized = usable(size.width) && usable(size.height);
        match self.state {
            SavedWindowState::Windowed => sized.then_some(WindowBounds::Windowed(self.bounds)),
            SavedWindowState::Maximized => {
                Some(WindowBounds::Maximized(self.restore_bounds(sized)))
            }
            SavedWindowState::Fullscreen => {
                Some(WindowBounds::Fullscreen(self.restore_bounds(sized)))
            }
        }
    }

    fn restore_bounds(self, sized: bool) -> Bounds<Pixels> {
        if sized {
            self.bounds
        } else {
            Bounds {
                origin: gpui::point(gpui::px(50.), gpui::px(50.)),
                size: gpui::size(gpui::px(1600.), gpui::px(900.)),
            }
        }
    }
}

/// The window geometry saved for `project_root`, to open its window with.
/// Whether it is still on a connected display is checked when the window is
/// opened.
pub(crate) fn saved_window_bounds(project_root: &Path) -> Option<WindowBounds> {
    read_slot(&layout_path(project_root), PRIMARY_SLOT)?
        .window?
        .to_window_bounds()
}

#[cfg(test)]
mod tests {
    use super::super::format::{write_slot, SavedWindowLayout};
    use super::*;
    use gpui::{point, px, size};

    #[test]
    fn window_geometry_round_trips_with_its_state() {
        let restore = Bounds {
            origin: point(px(120.), px(80.)),
            size: size(px(1500.), px(900.)),
        };
        let dir = std::env::temp_dir().join(format!("pulsar-layout-w-{}", std::process::id()));
        let layout = SavedWindowLayout {
            window: Some(SavedWindow::capture(WindowBounds::Maximized(restore), None)),
            ..Default::default()
        };
        write_slot(&layout_path(&dir), PRIMARY_SLOT, &layout).unwrap();

        // Maximized stays maximized and keeps its restore size.
        assert_eq!(
            saved_window_bounds(&dir),
            Some(WindowBounds::Maximized(restore))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fullscreen_reopens_fullscreen_whatever_its_saved_size() {
        // A fullscreen window reports screen-sized bounds; the saved size is
        // the windowed one to return to, and even garbage there must not stop
        // it reopening in full screen.
        let screen = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(2560.), px(1440.)),
        };
        let windowed = Bounds {
            origin: point(px(200.), px(100.)),
            size: size(px(1400.), px(800.)),
        };

        let saved = SavedWindow::capture(WindowBounds::Fullscreen(screen), Some(windowed));
        assert_eq!(saved.state, SavedWindowState::Fullscreen);
        assert_eq!(saved.bounds, windowed, "saves the size to return to");
        assert_eq!(
            saved.to_window_bounds(),
            Some(WindowBounds::Fullscreen(windowed))
        );

        let garbage = SavedWindow {
            state: SavedWindowState::Fullscreen,
            bounds: Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(f32::NAN), px(1.)),
            },
        };
        assert!(matches!(
            garbage.to_window_bounds(),
            Some(WindowBounds::Fullscreen(_))
        ));
    }

    #[test]
    fn an_implausible_window_size_is_not_used() {
        let tiny = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(10.), px(10.)),
        };
        assert!(SavedWindow::capture(WindowBounds::Windowed(tiny), None)
            .to_window_bounds()
            .is_none());
        let nan = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(f32::NAN), px(900.)),
        };
        assert!(SavedWindow::capture(WindowBounds::Windowed(nan), None)
            .to_window_bounds()
            .is_none());
    }
}
