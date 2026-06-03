//! PulsarWindowExt — open any PulsarWindow via WindowManager with Root theming.
//!
//! Import the trait and call `TypeName::open(params, cx)`. All routing goes
//! through the WindowManager so hooks, telemetry, and tracking apply uniformly.

use gpui::{App, AppContext as _, UpdateGlobal as _};
use ui::Root;
use window_manager::{PulsarWindow, WindowManager, WindowRequest};

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
}

impl<W: PulsarWindow> PulsarWindowExt for W {}
