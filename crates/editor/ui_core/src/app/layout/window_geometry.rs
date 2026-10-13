//! Saved window geometry: size, position and maximized / fullscreen state.
//!
//! Shared by the main window and pop-out windows. [`fit_to_displays`] checks a
//! restored secondary window against the connected displays (the main window is
//! checked when it is opened, in `ui_common::open_window`).

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

// ── Display validation ───────────────────────────────────────────────────────

/// Size of a pop-out whose saved size is unusable or whose display is gone.
const FALLBACK_POPOUT_SIZE: (f32, f32) = (800.0, 600.0);
/// Distance from the display's top-left corner of a fallback window.
const FALLBACK_INSET: f32 = 80.0;

fn bounds_in(b: &Bounds<Pixels>) -> (f32, f32, f32, f32) {
    (
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height),
    )
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds {
        origin: gpui::point(gpui::px(x), gpui::px(y)),
        size: gpui::size(gpui::px(w), gpui::px(h)),
    }
}

/// Whether a window at `bounds` is reachable on one of `displays`: its centre
/// lies on a display. Returns that display.
pub(crate) fn display_holding<'a>(
    bounds: &Bounds<Pixels>,
    displays: &'a [Bounds<Pixels>],
) -> Option<&'a Bounds<Pixels>> {
    let (x, y, w, h) = bounds_in(bounds);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    if !(cx.is_finite() && cy.is_finite()) {
        return None;
    }
    displays.iter().find(|d| {
        let (dx, dy, dw, dh) = bounds_in(d);
        cx >= dx && cx < dx + dw && cy >= dy && cy < dy + dh
    })
}

/// The bounds to open a restored secondary window with, given what was saved
/// (`None` when the saved size was unusable) and the connected `displays`
/// (primary first).
///
/// A window whose centre is still on a display keeps its position and state,
/// with its size limited to that display and its origin nudged so the whole
/// window is on it (the title bar stays reachable). One whose display is gone
/// (or with nothing saved) opens windowed at the default size, inset from the
/// primary display's corner.
pub(crate) fn fit_to_displays(
    saved: Option<WindowBounds>,
    displays: &[Bounds<Pixels>],
) -> WindowBounds {
    let fallback = |display: Option<&Bounds<Pixels>>| {
        let (dx, dy, dw, dh) = display.map(bounds_in).unwrap_or((0.0, 0.0, f32::MAX, f32::MAX));
        let (w, h) = FALLBACK_POPOUT_SIZE;
        WindowBounds::Windowed(rect(
            dx + FALLBACK_INSET,
            dy + FALLBACK_INSET,
            w.min(dw - FALLBACK_INSET),
            h.min(dh - FALLBACK_INSET),
        ))
    };
    let Some(saved) = saved else {
        return fallback(displays.first());
    };
    let (WindowBounds::Windowed(b) | WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b)) =
        saved;
    let Some(display) = display_holding(&b, displays) else {
        return fallback(displays.first());
    };
    let (dx, dy, dw, dh) = bounds_in(display);
    let (x, y, w, h) = bounds_in(&b);
    let (w, h) = (w.min(dw), h.min(dh));
    let x = x.clamp(dx, dx + dw - w);
    let y = y.clamp(dy, dy + dh - h);
    let fitted = rect(x, y, w, h);
    match saved {
        WindowBounds::Windowed(_) => WindowBounds::Windowed(fitted),
        WindowBounds::Maximized(_) => WindowBounds::Maximized(fitted),
        WindowBounds::Fullscreen(_) => WindowBounds::Fullscreen(fitted),
    }
}

/// [`fit_to_displays`] against the displays connected now.
pub(crate) fn fit_to_connected_displays(
    saved: SavedWindow,
    cx: &gpui::App,
) -> WindowBounds {
    let mut displays: Vec<Bounds<Pixels>> = Vec::new();
    if let Some(primary) = cx.primary_display() {
        displays.push(primary.bounds());
    }
    for display in cx.displays() {
        let b = display.bounds();
        if !displays.contains(&b) {
            displays.push(b);
        }
    }
    fit_to_displays(saved.to_window_bounds(), &displays)
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

    fn display(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        rect(x, y, w, h)
    }

    #[test]
    fn a_window_on_a_connected_display_keeps_its_place() {
        let displays = [display(0., 0., 1920., 1080.), display(1920., 0., 1920., 1080.)];
        let on_second = rect(2000., 100., 800., 600.);
        assert_eq!(
            fit_to_displays(Some(WindowBounds::Windowed(on_second)), &displays),
            WindowBounds::Windowed(on_second)
        );
        // Maximized keeps its state.
        assert_eq!(
            fit_to_displays(Some(WindowBounds::Maximized(on_second)), &displays),
            WindowBounds::Maximized(on_second)
        );
    }

    #[test]
    fn a_window_is_limited_to_and_nudged_inside_its_display() {
        let displays = [display(0., 0., 1000., 700.)];
        // Hangs off the right/bottom edges and is too wide.
        let big = rect(200., 400., 1400., 400.);
        let WindowBounds::Windowed(fit) = fit_to_displays(Some(WindowBounds::Windowed(big)), &displays)
        else {
            panic!("stays windowed");
        };
        assert_eq!(bounds_in(&fit), (0., 300., 1000., 400.));
    }

    #[test]
    fn a_window_on_a_vanished_display_opens_at_default_on_the_primary() {
        let displays = [display(0., 0., 1920., 1080.)];
        let gone = rect(3000., 200., 900., 700.);
        for state in [
            WindowBounds::Windowed(gone),
            WindowBounds::Maximized(gone),
            WindowBounds::Fullscreen(gone),
        ] {
            assert_eq!(
                fit_to_displays(Some(state), &displays),
                WindowBounds::Windowed(rect(80., 80., 800., 600.))
            );
        }
        // Nothing saved: same default.
        assert_eq!(
            fit_to_displays(None, &displays),
            WindowBounds::Windowed(rect(80., 80., 800., 600.))
        );
    }

    #[test]
    fn the_fallback_fits_a_small_display_and_survives_no_displays() {
        let tiny = [display(100., 50., 500., 400.)];
        assert_eq!(
            fit_to_displays(None, &tiny),
            WindowBounds::Windowed(rect(180., 130., 420., 320.))
        );
        assert_eq!(
            fit_to_displays(None, &[]),
            WindowBounds::Windowed(rect(80., 80., 800., 600.))
        );
        // A NaN position is not on any display.
        assert!(display_holding(&rect(f32::NAN, 0., 10., 10.), &tiny).is_none());
    }
}
