//! PulsarWindowExt — open any PulsarWindow via WindowManager with Root theming.
//!
//! Import the trait and call `TypeName::open(params, cx)`. All routing goes
//! through the WindowManager so hooks, telemetry, and tracking apply uniformly.

use gpui::{App, AppContext as _, Bounds, UpdateGlobal as _, WindowBounds, WindowOptions};
use ui::Root;
use window_manager::{apply_window_wrapper, PulsarWindow, WindowManager, WindowRegistry};

/// Extends every [`PulsarWindow`] with an `open` method that routes through
/// the [`WindowManager`] and wraps the entity in [`Root`] for theming.
///
/// # Example
/// ```ignore
/// use ui_common::PulsarWindowExt as _;
///
/// ProblemsWindow::open(drawer, cx);
/// SettingsWindow::open((), cx);
/// PulsarRoot::open(project_path, cx);
/// ```
pub trait PulsarWindowExt: PulsarWindow {
    /// Open this window through the [`WindowManager`], wrapped in [`Root`] for theming.
    /// If a window with the same `window_name()` already exists, it is focused instead.
    fn open(params: Self::Params, cx: &mut App) {
        let request = Self::window_request(&params);

        // Dedup: focus existing window if one is already open
        if WindowManager::update_global(cx, |wm, cx| wm.focus_window_by_request(&request, cx)) {
            return;
        }

        let profile = Self::window_profile(&params);
        let mut options = Self::window_options(&params);
        // Center windowed windows on the primary display
        options.window_bounds = options.window_bounds.map(|b| match b {
            WindowBounds::Windowed(bounds) => WindowBounds::centered(bounds.size, cx),
            other => other,
        });
        let _ = WindowManager::update_global(cx, |wm, cx| {
            if let Some(profile) = profile {
                let wrapper_kind = profile.wrapper();
                let profile_options = profile.options();
                wm.create_window(
                    request,
                    profile_options,
                    move |window, cx| {
                        let entity = Self::build(params, window, cx);
                        let wrapped = apply_window_wrapper(wrapper_kind, entity.into(), window, cx);
                        cx.new(|cx| Root::new(wrapped, window, cx))
                    },
                    cx,
                )
            } else {
                wm.create_window(
                    request,
                    options,
                    move |window, cx| {
                        let entity = Self::build(params, window, cx);
                        cx.new(|cx| Root::new(entity.into(), window, cx))
                    },
                    cx,
                )
            }
        });
    }

    /// Register this window in the [`WindowRegistry`] so it can be opened by name.
    ///
    /// Only available for windows whose `Params` implement `Default` (i.e. zero-param
    /// windows like Settings, About, etc.). Call once from `init()`.
    fn register(cx: &mut App)
    where
        Self::Params: Default,
    {
        WindowRegistry::update_global(cx, |reg, _| {
            reg.register(Self::window_name(), |cx| {
                Self::open(Self::Params::default(), cx);
            });
        });
    }
}

impl<W: PulsarWindow> PulsarWindowExt for W {}
