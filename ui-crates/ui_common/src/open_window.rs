//! PulsarWindowExt — open any PulsarWindow via WindowManager with Root theming.
//!
//! Import the trait and call `TypeName::open(params, cx)`. All routing goes
//! through the WindowManager so hooks, telemetry, and tracking apply uniformly.

use gpui::{App, AppContext as _, UpdateGlobal as _};
use ui::Root;
use window_manager::{PulsarWindow, WindowManager, WindowRegistry, WindowRequest};

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
    fn open(params: Self::Params, cx: &mut App) {
        let options = Self::window_options(&params);
        let _ = WindowManager::update_global(cx, |wm, cx| {
            wm.create_window(
                WindowRequest::Custom { type_name: Self::window_name() },
                options,
                move |window, cx| {
                    let entity = Self::build(params, window, cx);
                    cx.new(|cx| Root::new(entity.into(), window, cx))
                },
                cx,
            )
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
